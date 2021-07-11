use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use log::{error, info};
use rumqttc::{
    AsyncClient, ConnAck, Event, LastWill, MqttOptions, Outgoing, Packet, QoS, TlsConfiguration,
    Transport,
};

use crate::settings::{MqttTransport, Settings};

pub struct MqttHandle {
    pub active: tokio::sync::watch::Sender<bool>,
    pub application: tokio::sync::watch::Sender<String>,
}

impl MqttHandle {
    pub fn set_active(&mut self, active: bool) -> Result<()> {
        self.active
            .send(active)
            .map_err(|_| anyhow!("Failed to send message"))
    }
    pub fn set_application(&mut self, name: String) -> Result<()> {
        self.application
            .send(name)
            .map_err(|_| anyhow!("Failed to send message"))
    }
}

#[derive(Clone)]
pub struct State {
    pub active: tokio::sync::watch::Receiver<bool>,
    pub application: tokio::sync::watch::Receiver<String>,
}

pub async fn mqtt_loop(settings: &Settings, mut state: State) -> Result<()> {
    let power_topic = format!("{}/{}/power", settings.prefix, settings.id);
    let active_topic = format!("{}/{}/active", settings.prefix, settings.id);
    let application_topic = format!("{}/{}/application", settings.prefix, settings.id);

    let port = settings
        .mqtt
        .port
        .unwrap_or_else(|| match settings.mqtt.transport {
            MqttTransport::Tcp => 1883,
            MqttTransport::Tls => 8883,
        });
    let mut mqtt_options = MqttOptions::new(&settings.id, &settings.mqtt.host, port);
    match settings.mqtt.transport {
        MqttTransport::Tcp => mqtt_options.set_transport(Transport::Tcp),
        MqttTransport::Tls => {
            let mut config = rumqttc::ClientConfig::new();
            config.root_store =
                rustls_native_certs::load_native_certs().unwrap_or_else(|(partial, error)| {
                    partial
                        .ok_or_else(|| anyhow!("Failed to load certificate store {:?}", error))
                        .unwrap()
                });
            mqtt_options.set_transport(Transport::Tls(TlsConfiguration::Rustls(Arc::new(config))))
        }
    };
    if let Some(credentials) = &settings.mqtt.credentials {
        mqtt_options.set_credentials(&credentials.username, &credentials.password);
    }
    mqtt_options.set_last_will(LastWill::new(&power_topic, "OFF", QoS::AtLeastOnce, true));

    // Set capacity to 1.
    // Backpressure is handled more intelligently and for this application it just
    // doesn't make sense to buffer multiple values for the same topic.
    let (client, mut event_loop) = AsyncClient::new(mqtt_options, 1);

    let (connect_send, mut connect_receive) = tokio::sync::mpsc::channel(1);
    let event_loop = tokio::spawn(async move {
        // Keep this separate from the `publish(..).await`s.
        // There's an in-memory queue that holds messages until they are dispatched from
        // this coroutine. If that queue fills up, `publish(..).await` will pause the
        // coroutine until this coroutine makes progress emptying the queue. If they're
        // the same coroutine the code will deadlock as soon as the queue overflows.
        const MIN_DELAY: Duration = Duration::from_secs(1);
        let mut start = Instant::now();
        let mut stop = false;
        loop {
            match event_loop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ConnAck {
                    code: rumqttc::ConnectReturnCode::Success,
                    ..
                }))) => {
                    info!("MQTT connected");
                    // LWT sets power to off on disconnect so we need to set power to on
                    // after every connect.
                    // Don't do it from this coroutine or the code can deadlock.
                    let _ = connect_send.try_send(());
                }
                Ok(Event::Outgoing(Outgoing::Disconnect)) => {
                    stop = true;
                }
                Ok(_) => {}
                Err(error) => {
                    if stop {
                        break;
                    }
                    error!("MQTT error: {:?}", error);

                    // Wait so we don't flood the network with requests and then try again.
                    let elapsed = start.elapsed();
                    if elapsed < MIN_DELAY {
                        tokio::time::sleep(MIN_DELAY - elapsed).await;
                    }
                    start = Instant::now();
                }
            }
        }
    });

    if !settings.hass_prefix.is_empty() {
        client
            .publish(
                format!(
                    "{}/binary_sensor/{}_power/config",
                    settings.hass_prefix, settings.id
                ),
                QoS::AtLeastOnce,
                true,
                serde_json::to_string(&serde_json::json!({
                    "name": format!("{} Power", settings.name),
                    "device_class": "power",
                    "state_topic": &power_topic,
                }))
                .unwrap(),
            )
            .await?;
        client
            .publish(
                format!(
                    "{}/binary_sensor/{}_active/config",
                    settings.hass_prefix, settings.id
                ),
                QoS::AtLeastOnce,
                true,
                serde_json::to_string(&serde_json::json!({
                    "name": format!("{} Active", settings.name),
                    "device_class": "moving",
                    "state_topic": &active_topic,
                    "availability": [{
                        "topic": &power_topic,
                        "payload_available": "ON",
                        "payload_not_available": "OFF",
                    }],
                }))
                .unwrap(),
            )
            .await?;
        client
            .publish(
                format!(
                    "{}/sensor/{}_application/config",
                    settings.hass_prefix, settings.id
                ),
                QoS::AtLeastOnce,
                true,
                serde_json::to_string(&serde_json::json!({
                    "name": format!("{} Application", settings.name),
                    "state_topic": &application_topic,
                    "availability": [{
                        "topic": &power_topic,
                        "payload_available": "ON",
                        "payload_not_available": "OFF",
                    }],
                }))
                .unwrap(),
            )
            .await?;
    }

    loop {
        tokio::select! {
            recv = connect_receive.recv() => {
                if recv.is_some() {
                    client.publish(&power_topic, QoS::AtLeastOnce, true, "ON").await?;
                } else {
                    break;
                }
            }
            recv = state.active.changed() => {
                if recv.is_err() {
                    break;
                }
                let active = state.active.borrow_and_update();
                client.publish(&active_topic, QoS::AtLeastOnce, true, if *active { "ON" } else { "OFF" }).await?;
            }
            recv = state.application.changed() => {
                if recv.is_err() {
                    break;
                }
                let application = state.application.borrow_and_update();
                client.publish(&application_topic, QoS::AtLeastOnce, true, application.as_str()).await?;
            }
        }
    }

    client.disconnect().await?;

    event_loop.await?;

    Ok(())
}
