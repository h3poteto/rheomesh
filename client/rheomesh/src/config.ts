const EXTMAP_ORDER: { [key: string]: number } = {
  "urn:ietf:params:rtp-hdrext:ssrc-audio-level": 1,
  "http://www.webrtc.org/experiments/rtp-hdrext/abs-send-time": 2,
  "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01": 3,
  "urn:ietf:params:rtp-hdrext:sdes:mid": 4,
  "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay": 5,
  "http://www.webrtc.org/experiments/rtp-hdrext/video-content-type": 6,
  "http://www.webrtc.org/experiments/rtp-hdrext/video-timing": 7,
  "http://www.webrtc.org/experiments/rtp-hdrext/color-space": 8,
  "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id": 10,
  "urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id": 11,
  "https://aomediacodec.github.io/av1-rtp-spec/#dependency-descriptor-rtp-header-extension": 12,
  "urn:3gpp:video-orientation": 13,
  "urn:ietf:params:rtp-hdrext:toffset": 14,
  "http://www.webrtc.org/experiments/rtp-hdrext/video-layers-allocation00": 15,
};

export function findExtmapOrder(uri: string): number | null {
  const order = EXTMAP_ORDER[uri];
  if (order) {
    return order;
  } else {
    return null;
  }
}
