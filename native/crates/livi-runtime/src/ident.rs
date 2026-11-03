use iap2_csm::messages::identification::*;

use crate::vehicle::{Fuels, Plugs};

#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub ssid: String,
    pub bt_mac: [u8; 6],
    pub fuels: Fuels,
    pub plugs: Plugs,
    pub theme: Option<Theme>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub id: String,
    pub version: u32,
    pub sdk_version: String,
}

impl Theme {
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.trim().split(':');
        let id = parts.next().filter(|id| !id.is_empty())?.to_string();
        let version = parts.next()?.parse().ok()?;
        let sdk_version = parts.next().filter(|v| !v.is_empty())?.to_string();
        Some(Self {
            id,
            version,
            sdk_version,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Wireless,
    Wired,
}

fn ids(msgs: &[u16]) -> Vec<u8> {
    msgs.iter().flat_map(|m| m.to_be_bytes()).collect()
}

const SENT: &[u16] = &[
    0xA101, 0x5703, 0x5000, 0x5002, 0x5003, 0x5200, 0x5203, 0xFFFB, 0xAE00, 0xAE02, 0x4157, 0x4159,
    0x4154, 0x4156, 0x4301,
];

const RECEIVED: &[u16] = &[
    0xEA00, 0xEA01, 0xA100, 0xA102, 0x4E0D, 0x4E0E, 0x5702, 0x5001, 0x5201, 0x5202, 0xFFFA, 0xFFFC,
    0xAE01, 0x4158, 0x4155, 0x4E0B, 0x4300,
];

const POWER_SOURCE_UPDATE: u16 = 0xAE03;

pub const DROPPABLE: &[&str] = &[
    "location_information_component",
    "vehicle_information_component",
    "vehicle_status_component",
    "route_guidance_display_component",
    CALL_MESSAGES,
];

/// The messages of call control and the call lists (crate::calls), left out again when the phone refuses the
/// lists of messages
pub const CALL_MESSAGES: &str = "call_messages";

/// `calls`: the session takes part in calls (CpConfig::calls), so the identification names their messages unless
/// `exclude` has CALL_MESSAGES
pub fn build_identification(
    id: &Identity,
    transport: Transport,
    exclude: &[&str],
    calls: bool,
) -> IdentificationInformation {
    let dropped = |f: &str| exclude.contains(&f);
    let wired = transport == Transport::Wired;
    let mut sent = SENT.to_vec();
    if wired {
        sent.push(POWER_SOURCE_UPDATE);
    }
    let mut received = RECEIVED.to_vec();
    if calls && !dropped(CALL_MESSAGES) {
        sent.extend_from_slice(crate::calls::SENT);
        received.extend_from_slice(crate::calls::RECEIVED);
    }
    IdentificationInformation {
        name: id.name.clone(),
        model_identifier: "LIVI".into(),
        manufacturer: "LIVI".into(),
        serial_number: "0123456".into(),
        firmware_version: "1.0.0".into(),
        hardware_version: "1.0".into(),
        messages_sent_by_accessory: ids(&sent),
        messages_received_from_accessory: ids(&received),
        power_providing_capability: if wired {
            PowerProvidingCapability::Advanced
        } else {
            PowerProvidingCapability::None
        },
        maximum_current_drawn_from_device: 20,
        supported_external_accessory_protocol: vec![ExternalAccessoryProtocol {
            id: 1,
            name: "dev.f-io.livi".into(),
            match_action: MatchAction::NoActionNoCommunication,
            native_transport_component_identifier: None,
            car_play: false,
        }],
        app_match_team_id: None,
        current_language: "en".into(),
        supported_language: vec!["en".into(), "de".into()],
        serial_transport_component: vec![],
        usb_device_transport_component: vec![],
        usb_host_transport_component: if wired {
            vec![USBHostTransportComponent {
                id: 0,
                name: "USBHostTransport".into(),
                supports_iap2_connection: true,
                car_play_interface_number: Some(
                    std::env::var("LIVI_CP_USB_IFACE")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(3),
                ),
                supports_car_play: true,
                theme_asset_identifier: id.theme.as_ref().map(|t| t.id.clone()),
            }]
        } else {
            vec![]
        },
        bluetooth_transport_component: if wired {
            vec![]
        } else {
            vec![BluetoothTransportComponent {
                id: 0,
                name: "blue".into(),
                supports_iap2_connection: true,
                bluetooth_transport_mac: id.bt_mac.to_vec(),
            }]
        },
        vehicle_information_component: if dropped("vehicle_information_component") {
            None
        } else {
            Some(VehicleInformationComponent {
                id: 0,
                name: id.name.clone(),
                engine_types: id.fuels.engine_types(),
                display_name: Some(id.name.clone()),
                maps_display_name: Some(id.name.clone()),
                supported_charging_connectors: id.plugs.types(),
                power_ccs1_w: id.plugs.watts(ChargingConnectorType::Ccs1),
                power_ccs2_w: id.plugs.watts(ChargingConnectorType::Ccs2),
                power_j1772_w: id.plugs.watts(ChargingConnectorType::J1772),
                power_chademo_w: id.plugs.watts(ChargingConnectorType::Chademo),
                power_mennekes_w: id.plugs.watts(ChargingConnectorType::Mennekes),
                power_gbt_dc_w: id.plugs.watts(ChargingConnectorType::GbtDc),
                power_gbt_ac_w: id.plugs.watts(ChargingConnectorType::GbtAc),
                power_nacs_dc_w: id.plugs.watts(ChargingConnectorType::NacsDc),
                power_nacs_ac_w: id.plugs.watts(ChargingConnectorType::NacsAc),
            })
        },
        vehicle_status_component: if dropped("vehicle_status_component") {
            None
        } else {
            Some(VehicleStatusComponent {
                id: 0,
                name: id.name.clone(),
                range: true,
                outside_temperature: true,
                range_warning: true,
                range_gasoline: id.fuels.gasoline,
                range_diesel: id.fuels.diesel,
                range_electric: id.fuels.electric,
                range_cng: id.fuels.cng,
                range_warning_gasoline: false,
                range_warning_diesel: false,
                range_warning_electric: false,
                range_warning_cng: false,
                electric_charge_info: id.fuels.electric,
                max_range_info: id.fuels.electric,
            })
        },
        location_information_component: if dropped("location_information_component") {
            None
        } else {
            Some(LocationInformationComponent {
                id: 0,
                name: id.name.clone(),
                global_positioning_system_fix_data: true,
                recommended_minimum_specific_gps_transit_data: true,
            })
        },
        wireless_car_play_transport_component: if wired {
            None
        } else {
            Some(WirelessCarPlayTransportComponent {
                id: 1,
                name: id.ssid.clone(),
                supports_iap2_connection: true,
                supports_car_play: true,
                theme_asset_identifier: id.theme.as_ref().map(|t| t.id.clone()),
            })
        },
        route_guidance_display_component: if dropped("route_guidance_display_component") {
            vec![]
        } else {
            vec![RouteGuidanceDisplayComponent {
                id: 0,
                name: id.name.clone(),
                max_current_road_name_length: Some(128),
                max_destination_name_length: Some(128),
                max_after_maneuver_road_name_length: Some(128),
                max_maneuver_description_length: Some(128),
                max_guidance_maneuver_storage_capacity: Some(0),
                max_lane_guidance_description_length: None,
                max_lane_guidance_storage_capacity: None,
            }]
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iap2_csm::CsmMessage;

    /// The lists of messages as they were before calls (1498af94 and 09264412)
    const SENT_BEFORE: &[u16] = &[
        0xA101, 0x5703, 0x5000, 0x5002, 0x5003, 0x5200, 0x5203, 0xFFFB, 0xAE00, 0xAE02, 0x4157,
        0x4159, 0x4154, 0x4156, 0x4301,
    ];
    const RECEIVED_BEFORE: &[u16] = &[
        0xEA00, 0xEA01, 0xA100, 0xA102, 0x4E0D, 0x4E0E, 0x5702, 0x5001, 0x5201, 0x5202, 0xFFFA,
        0xFFFC, 0xAE01, 0x4158, 0x4155, 0x4E0B, 0x4300,
    ];

    fn identity() -> Identity {
        Identity {
            name: "LIVI".into(),
            ssid: "LIVI".into(),
            bt_mac: [0xAA, 0xBB, 0xCC, 0x11, 0x22, 0x33],
            fuels: Default::default(),
            plugs: Default::default(),
            theme: None,
        }
    }

    #[test]
    fn without_calls_the_identification_is_byte_for_byte_the_one_before() {
        for (transport, power) in [
            (Transport::Wireless, &[][..]),
            (Transport::Wired, &[0xAE03][..]),
        ] {
            let id = identity();
            let off = build_identification(&id, transport, &[], false);
            assert_eq!(
                off.messages_sent_by_accessory,
                ids(&[SENT_BEFORE, power].concat())
            );
            assert_eq!(off.messages_received_from_accessory, ids(RECEIVED_BEFORE));
            // the switch changes the lists of messages and nothing else
            let mut on = build_identification(&id, transport, &[], true);
            assert_ne!(on.encode(), off.encode());
            on.messages_sent_by_accessory = off.messages_sent_by_accessory.clone();
            on.messages_received_from_accessory = off.messages_received_from_accessory.clone();
            assert_eq!(on.encode(), off.encode());
            // refused lists: the identification goes back to the one without calls
            let dropped = build_identification(&id, transport, &[CALL_MESSAGES], true);
            assert_eq!(dropped.encode(), off.encode());
        }
    }
}
