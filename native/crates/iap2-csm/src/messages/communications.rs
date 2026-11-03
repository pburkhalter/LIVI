use crate::{csm_group, csm_message};

csm_message! {
    pub struct StartCallStateUpdates = 0x4154 {
        0 => remote_id: [flag],
        1 => display_name: [flag],
        2 => status: [flag],
        3 => direction: [flag],
        4 => call_uuid: [flag],
        6 => address_book_id: [flag],
        7 => label: [flag],
        8 => service: [flag],
        9 => is_conferenced: [flag],
        10 => conference_group: [flag],
        11 => disconnect_reason: [flag],
        12 => start_timestamp: [flag],
    }
}

csm_message! {
    pub struct CallStateUpdate = 0x4155 {
        0 => remote_id: [opt str],
        1 => display_name: [opt str],
        2 => status: [opt u8],
        3 => direction: [opt u8],
        4 => call_uuid: [opt str],
        11 => disconnect_reason: [opt u8],
    }
}

csm_message! {
    pub struct StopCallStateUpdates = 0x4156 {}
}

csm_message! {
    pub struct StartCommunicationsUpdates = 0x4157 {
        0 => signal_strength: [flag],
        1 => registration_status: [flag],
        2 => airplane_mode_status: [flag],
        4 => carrier_name: [flag],
        5 => cellular_supported: [flag],
    }
}

csm_message! {
    pub struct CommunicationsUpdate = 0x4158 {
        0 => signal_strength: [opt bytes],
        1 => registration_status: [opt bytes],
        2 => airplane_mode_status: [opt bool],
        4 => carrier_name: [opt str],
        5 => cellular_supported: [opt bool],
    }
}

csm_message! {
    pub struct StopCommunicationsUpdates = 0x4159 {}
}

// Call control and the lists of recent and favourite calls. The parameters are the ones a CarPlay head unit on a
// car uses with iOS 26/27 (luka-dev/mib2q-carplay-rgi, docs/cluster-and-rgi/IAP2_COMMUNICATIONS.md, read from iOS's
// accessoryd and confirmed on the car; ProManiFest2122/mhi2-k3346-carplay-vc for InitiateCall). Without a call UUID
// the phone takes the ringing call (AcceptCall) or the frontmost one (EndCall, HoldStatusUpdate, MuteStatusUpdate).
// MergeCalls (0x415E) and SendDTMF (0x4161) have no layout anyone published; the tones go as CarPlay telephony keys.

csm_message! {
    /// `call_type`: 0 a destination, 1 the voicemail, 2 the last number again. `service` (only with a destination):
    /// 1 telephony, 2 FaceTime audio, 3 FaceTime video
    pub struct InitiateCall = 0x415A {
        0 => call_type: [u8],
        1 => destination_id: [opt str],
        2 => service: [opt u8],
        3 => address_book_id: [opt str],
    }
}

csm_message! {
    /// `action`: 0 accept (holding an active call), 1 end the active call and accept
    pub struct AcceptCall = 0x415B {
        0 => action: [u8],
        1 => call_uuid: [opt str],
    }
}

csm_message! {
    /// `action`: 0 end this call (the frontmost without a UUID, also while it rings or dials), 1 end all
    pub struct EndCall = 0x415C {
        0 => action: [u8],
        1 => call_uuid: [opt str],
    }
}

csm_message! {
    pub struct SwapCalls = 0x415D {}
}

csm_message! {
    pub struct HoldStatusUpdate = 0x415F {
        0 => hold_status: [bool],
        1 => call_uuid: [opt str],
    }
}

csm_message! {
    /// Mutes the microphone of the frontmost call; nothing without a call
    pub struct MuteStatusUpdate = 0x4160 {
        0 => mute_status: [bool],
    }
}

csm_group! {
    /// Which fields of a recent call the phone sends (all empty parameters)
    pub struct RecentsListProperties {
        0 => index: [flag],
        1 => remote_id: [flag],
        2 => display_name: [flag],
        3 => label: [flag],
        4 => address_book_id: [flag],
        5 => service: [flag],
        6 => kind: [flag],
        7 => unix_timestamp: [flag],
        8 => duration: [flag],
        9 => occurrences: [flag],
    }
}

csm_group! {
    pub struct FavoritesListProperties {
        0 => index: [flag],
        1 => remote_id: [flag],
        2 => display_name: [flag],
        3 => label: [flag],
        4 => address_book_id: [flag],
        5 => service: [flag],
    }
}

csm_message! {
    /// `recents_max`/`favorites_max`: at most that many entries; iOS caps them at 12 for CarPlay, 0 means that cap.
    /// `recents_combine`: one entry per number, with `occurrences`
    pub struct StartListUpdates = 0x4170 {
        0 => recents_available: [flag],
        1 => recents_properties: [opt group RecentsListProperties],
        2 => recents_count: [flag],
        3 => recents_max: [opt u16],
        4 => recents_combine: [opt bool],
        5 => favorites_available: [flag],
        6 => favorites_properties: [opt group FavoritesListProperties],
        7 => favorites_count: [flag],
        8 => favorites_max: [opt u16],
    }
}

csm_message! {
    pub struct StopListUpdates = 0x4172 {}
}

csm_group! {
    /// One recent or favourite call at its absolute `index`. `display_name` is the contact's name, or the number
    /// written out when there is no contact. `kind` (recents only): 1 incoming, 2 outgoing, 3 missed, 0 other.
    /// `service`: 1 telephony, 2 FaceTime audio, 3 FaceTime video, 0 other
    pub struct ListEntry {
        0 => index: [opt u16],
        1 => remote_id: [opt str],
        2 => display_name: [opt str],
        3 => label: [opt str],
        4 => address_book_id: [opt str],
        5 => service: [opt u8],
        6 => kind: [opt u8],
        7 => unix_timestamp: [opt u64],
        8 => duration: [opt u32],
        9 => occurrences: [opt u8],
    }
}

/// The phone sends its whole (capped) lists again on every change, spread over as many messages as their size needs:
/// the first one with whether a list is there and its length, the others with entries only. An entry that does not
/// decode is left out, so that one odd field does not take the others with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListUpdate {
    pub recents_available: Option<bool>,
    pub recents: Vec<ListEntry>,
    pub recents_count: Option<u16>,
    pub favorites_available: Option<bool>,
    pub favorites: Vec<ListEntry>,
    pub favorites_count: Option<u16>,
}

fn flag_of(b: &[u8], param: &'static str) -> Result<bool, crate::Error> {
    match b {
        [v] => Ok(*v != 0),
        _ => Err(crate::Error::Scalar { param }),
    }
}

fn count_of(b: &[u8], param: &'static str) -> Result<u16, crate::Error> {
    <[u8; 2]>::try_from(b)
        .map(u16::from_be_bytes)
        .map_err(|_| crate::Error::Scalar { param })
}

impl crate::CsmParams for ListUpdate {
    fn encode_params(&self, out: &mut Vec<u8>) {
        let lists = [
            (
                0,
                1,
                2,
                self.recents_available,
                &self.recents,
                self.recents_count,
            ),
            (
                5,
                6,
                7,
                self.favorites_available,
                &self.favorites,
                self.favorites_count,
            ),
        ];
        for (available_id, entry_id, count_id, available, entries, count) in lists {
            if let Some(a) = available {
                crate::put_param(out, available_id, &[u8::from(a)]);
            }
            for e in entries {
                let mut p = Vec::new();
                crate::CsmParams::encode_params(e, &mut p);
                crate::put_param(out, entry_id, &p);
            }
            if let Some(n) = count {
                crate::put_param(out, count_id, &n.to_be_bytes());
            }
        }
    }

    fn decode_params(params: &[(u16, &[u8])]) -> Result<Self, crate::Error> {
        let mut m = Self::default();
        for &(id, b) in params {
            match id {
                0 => m.recents_available = Some(flag_of(b, "recents_available")?),
                2 => m.recents_count = Some(count_of(b, "recents_count")?),
                5 => m.favorites_available = Some(flag_of(b, "favorites_available")?),
                7 => m.favorites_count = Some(count_of(b, "favorites_count")?),
                1 | 6 => {
                    let Ok(e) =
                        <ListEntry as crate::CsmParams>::decode_params(&crate::split_params(b))
                    else {
                        continue;
                    };
                    if id == 1 {
                        m.recents.push(e)
                    } else {
                        m.favorites.push(e)
                    }
                }
                _ => {}
            }
        }
        Ok(m)
    }
}

impl crate::CsmMessage for ListUpdate {
    const MSG_ID: u16 = 0x4171;
}
