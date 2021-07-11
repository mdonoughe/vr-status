mod mqtt;
mod openvr;
mod settings;

use std::{ffi::CStr, time::Duration};

use anyhow::{bail, Context, Result};
use bindings::{
    openvr::{
        EVRApplicationProperty_EVRApplicationProperty_VRApplicationProperty_Name_String,
        EVRApplicationType_EVRApplicationType_VRApplication_Background,
        EVREventType_EVREventType_VREvent_EnterStandbyMode,
        EVREventType_EVREventType_VREvent_LeaveStandbyMode, EVREventType_EVREventType_VREvent_Quit,
        EVREventType_EVREventType_VREvent_SceneApplicationChanged,
        EVREventType_EVREventType_VREvent_SceneApplicationStateChanged,
    },
    Windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};
use cstr::cstr;
use log::{debug, error, info};
use openvr::{VrApplications, VrSystem};

use crate::{
    mqtt::{mqtt_loop, MqttHandle, State},
    openvr::OpenVr,
    settings::load_settings,
};

async fn run() -> Result<()> {
    let settings = load_settings().await?;

    let id = cstr!("mdonoughe.VrStatus");
    let vr = OpenVr::new(EVRApplicationType_EVRApplicationType_VRApplication_Background)?;
    let system = vr.system()?;
    let applications = vr.applications()?;

    let mut path = ::std::env::current_exe().context("Could not find installation directory")?;
    path.pop();
    path.push("vr-status.vrmanifest\0");
    let path = path
        .to_str()
        .context("Invalid characters in installation path")?;
    let path = CStr::from_bytes_with_nul(path.as_bytes())
        .context("Null characters in installation path")?;

    if let Err(error) = applications.add_application_manifest(path, false) {
        bail!(
            "Failed to register application {}: {}",
            error,
            applications
                .get_applications_err_name_from_enum(error)
                .to_string_lossy()
        )
    }

    if !applications.get_application_auto_launch(id) {
        if let Err(error) = applications.set_application_auto_launch(id, true) {
            bail!(
                "Failed to enable auto launch {}: {}",
                error,
                applications
                    .get_applications_err_name_from_enum(error)
                    .to_string_lossy()
            )
        }
    }

    let (active_send, active_receive) = tokio::sync::watch::channel(true);
    let (application_send, application_receive) = tokio::sync::watch::channel(String::new());

    let mqtt = MqttHandle {
        active: active_send,
        application: application_send,
    };

    let state = State {
        active: active_receive,
        application: application_receive,
    };

    let main_future = main_loop(&system, &applications, mqtt);
    let mqtt_future = mqtt_loop(&settings, state);

    tokio::select! {
        result = main_future => result,
        result = mqtt_future => result,
    }
}

async fn main_loop<'a>(
    system: &VrSystem<'a>,
    applications: &VrApplications<'a>,
    mut mqtt: MqttHandle,
) -> Result<()> {
    loop {
        match system.poll_next_event() {
            Some(event) =>
            {
                #[allow(non_upper_case_globals)]
                match event.eventType as i32 {
                    EVREventType_EVREventType_VREvent_SceneApplicationChanged
                    | EVREventType_EVREventType_VREvent_SceneApplicationStateChanged => {
                        let pid = applications.get_current_scene_process_id();
                        if pid != 0 {
                            debug!("Active application pid is now {}", pid);
                            match applications
                                .get_application_key_by_process_id(pid)
                                .context("Failed to get application key")
                            {
                                Ok(key) => {
                                    debug!(
                                        "Active application key is now {}",
                                        key.to_string_lossy()
                                    );
                                    match applications.get_application_property_string(&key, EVRApplicationProperty_EVRApplicationProperty_VRApplicationProperty_Name_String).context("Failed to get application name") {
                                        Ok(name) => {
                                            info!("Active application is now {}", name);
                                            mqtt.set_application(name).context("Failed to queue application update")?;
                                        }
                                        Err(error) => {
                                            error!("Failed to retrieve application name: {:?}", error)
                                        }
                                    }
                                }
                                Err(error) => {
                                    error!("Failed to retrieve application key: {:?}", error)
                                }
                            }
                        }
                    }
                    EVREventType_EVREventType_VREvent_EnterStandbyMode => mqtt
                        .set_active(false)
                        .context("Failed to queue standby update")?,
                    EVREventType_EVREventType_VREvent_LeaveStandbyMode => mqtt
                        .set_active(true)
                        .context("Failed to queue standby update")?,
                    EVREventType_EVREventType_VREvent_Quit => {
                        system.acknowledge_quit_exiting();
                        break;
                    }
                    _ => {}
                }
            }
            None => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    if let Err(error) = run().await {
        unsafe {
            MessageBoxW(
                None,
                format!("{:?}", error),
                "vr-status",
                MB_OK | MB_ICONERROR,
            );
        }
    }
}
