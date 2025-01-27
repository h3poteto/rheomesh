const EXTMAP_ORDER: { [key: string]: number } = {
  "urn:ietf:params:rtp-hdrext:ssrc-audio-level": 1,
  "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time": 2,
  "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01": 3,
  "urn:ietf:params:rtp-hdrext:sdes:mid": 4,
  "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay": 5,
  "http://www.webrtc.org/experiments/rtp-hdrext/video-content-type": 6,
  "http://www.webrtc.org/experiments/rtp-hdrext/video-timing": 7,
  "http://www.webrtc.org/experiments/rtp-hdrext/color-space": 8,
  "urn:ietf:params:rtp-hdrext:framemarking": 9,
  "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id": 10,
  "urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id": 11,
  "https://aomediacodec.github.io/av1-rtp-spec/#dependency-descriptor-rtp-header-extension": 12,
  "urn:3gpp:video-orientation": 13,
  "urn:ietf:params:rtp-hdrext:toffset": 14,
  "http://www.webrtc.org/experiments/rtp-hdrext/video-layers-allocation00": 15,
};

const RID_LOW = "low";
const RID_MID = "mid";
const RID_HIGH = "high";

export function findExtmapOrder(uri: string): number | null {
  const order = EXTMAP_ORDER[uri];
  if (order) {
    return order;
  } else {
    return null;
  }
}

export function simulcastEncodings(): Array<RTCRtpEncodingParameters> {
  return [
    { rid: RID_LOW, maxBitrate: 125000, scaleResolutionDownBy: 4.0 },
    { rid: RID_MID, maxBitrate: 500000, scaleResolutionDownBy: 2.0 },
    { rid: RID_HIGH, maxBitrate: 2500000, scaleResolutionDownBy: 1.0 },
  ];
}

export interface SVCRTCRtpEncodingParameters extends RTCRtpEncodingParameters {
  scalabilityMode: string;
}

export function SVCEncodings(): Array<SVCRTCRtpEncodingParameters> {
  return [{ scalabilityMode: "L2T3_KEY" }];
}
