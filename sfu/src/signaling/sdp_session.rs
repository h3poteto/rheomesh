use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc_sdp::{
    SdpSession,
    attribute_type::{SdpAttribute, SdpAttributeType},
    parse_sdp,
};

use crate::error::{Error, WhipSdpErrorKind};

pub async fn get_current_ice(sdp: &RTCSessionDescription) -> Result<(String, String), Error> {
    let parsed = parse_sdp(&sdp.sdp, false).map_err(|e| {
        Error::new_whip_sdp(
            format!("Failed to parse local SDP: {}", e),
            WhipSdpErrorKind::FailedToGetRemoteSdpError,
        )
    })?;
    let ice_ufrag = get_ice_ufrag(&parsed)?;
    let ice_pwd = get_ice_pwd(&parsed)?;
    Ok((ice_ufrag, ice_pwd))
}

fn get_ice_ufrag(sdp: &SdpSession) -> Result<String, Error> {
    let ice_ufrag = sdp
        .get_attribute(SdpAttributeType::IceUfrag)
        .ok_or_else(|| {
            Error::new_whip_sdp(
                "Missing ice-ufrag in local SDP".to_string(),
                WhipSdpErrorKind::FailedToGetLocalIceError,
            )
        })?;
    match ice_ufrag {
        SdpAttribute::IceUfrag(ufrag) => Ok(ufrag.clone()),
        _ => Err(Error::new_whip_sdp(
            "Invalid ice-ufrag attribute type".to_string(),
            WhipSdpErrorKind::FailedToGetLocalIceError,
        )),
    }
}

fn get_ice_pwd(sdp: &SdpSession) -> Result<String, Error> {
    let ice_pwd = sdp.get_attribute(SdpAttributeType::IcePwd).ok_or_else(|| {
        Error::new_whip_sdp(
            "Missing ice-pwd in local SDP".to_string(),
            WhipSdpErrorKind::FailedToGetLocalIceError,
        )
    })?;
    match ice_pwd {
        SdpAttribute::IcePwd(pwd) => Ok(pwd.clone()),
        _ => Err(Error::new_whip_sdp(
            "Invalid ice-pwd attribute type".to_string(),
            WhipSdpErrorKind::FailedToGetLocalIceError,
        )),
    }
}

pub fn rfc8840(sdp: &RTCSessionDescription) -> Result<String, Error> {
    let result = sdp
        .sdp
        .lines()
        .filter(|line| {
            line.starts_with("a=ice-")
                || line.starts_with("a=group:BUNDLE")
                || line.starts_with("m=")
                || line.starts_with("a=mid:")
                || line.starts_with("a=candidate:")
                || line.starts_with("a=end-of-candidates")
        })
        .collect::<Vec<&str>>()
        .join("\r\n");
    Ok(result)
}
