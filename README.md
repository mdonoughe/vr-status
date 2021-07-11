# vr-status

> Reports OpenVR state to MQTT.

vr-status is an OpenVR plugin that reports changes in OpenVR state to an MQTT server so that you can drive home automation based on what you are doing in VR.

I use this for my setup.

## Configuration

See the file vr-status.yaml.

## Installation

Start SteamVR and then run vr-status.exe. It will register itself with SteamVR as an overlay that should start automatically in the future.

If you are using [Home Assistant] and have [MQTT discovery] enabled (enabled by default when you configure MQTT), entities will be automatically created within Home Assistant.

[Home Assistant]: https://www.home-assistant.io/
[MQTT discovery]: https://www.home-assistant.io/docs/mqtt/discovery/

## Uninstallation

SteamVR normally changes the following files during the installation process:

- `C:\Program Files (x86)\Steam\config\appconfig.json` gains an extra line with the path to vr-status.
- `C:\Program Files (x86)\Steam\config\vrappconfig\mdonoughe.VrStatus.vrappconfig` is created.

You can completely uninstall by reverting these changes, but it is probably sufficient to just delete vr-status.
