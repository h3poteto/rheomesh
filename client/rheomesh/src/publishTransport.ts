import { EventEmitter } from "events";
import * as sdpTransform from "sdp-transform";
import { findExtmapOrder } from "./config";

const offerOptions: RTCOfferOptions = {
  offerToReceiveVideo: false,
  offerToReceiveAudio: false,
};

export class PublishTransport extends EventEmitter {
  private _peerConnection: RTCPeerConnection;
  private _signalingLock: boolean;

  constructor(config: RTCConfiguration) {
    super();
    const peer = new RTCPeerConnection(config);

    this._peerConnection = peer;
    this._signalingLock = false;

    this._peerConnection.onicecandidate = (event) => {
      if (event.candidate) {
        this.emit("icecandidate", event.candidate);
      }
    };

    this._peerConnection.onsignalingstatechange = (event) => {
      console.debug(
        "onsignalingstatechange: ",
        (event.target as RTCPeerConnection).signalingState,
      );
      if ((event.target as RTCPeerConnection).signalingState === "stable") {
        this._signalingLock = false;
      }
    };

    this._peerConnection.onnegotiationneeded = async (_event) => {
      while (this._signalingLock) {
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      this._signalingLock = true;
      const offer = await this._peerConnection.createOffer(offerOptions);
      await this._peerConnection.setLocalDescription(offer);

      this.emit("negotiationneeded", offer);
    };
  }

  public async publish(
    track: MediaStreamTrack,
    encodings?: Array<RTCRtpEncodingParameters>,
  ): Promise<RTCSessionDescriptionInit> {
    while (this._signalingLock) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    this._signalingLock = true;

    let options: RTCRtpTransceiverInit = {
      direction: "sendonly",
    };
    if (track.kind === "video" && encodings) {
      options = Object.assign(options, {
        sendEncodings: encodings,
      });
    }
    this._peerConnection.addTransceiver(track, options);

    const init = await this._peerConnection.createOffer(offerOptions);
    const offer = adjustExtmap(init);
    console.log("offer:", offer);

    await this._peerConnection.setLocalDescription(offer);

    await this.waitForIceGatheringComplete(this._peerConnection);

    const res = this._peerConnection.localDescription;
    if (!res) {
      throw new Error("empty localDescription");
    }
    return res;
  }

  public async setAnswer(answer: RTCSessionDescription): Promise<void> {
    await this._peerConnection.setRemoteDescription(answer);
  }

  public async addIceCandidate(candidate: RTCIceCandidateInit): Promise<void> {
    await this._peerConnection.addIceCandidate(new RTCIceCandidate(candidate));
  }

  public async publishData(): Promise<
    [RTCDataChannel, RTCSessionDescriptionInit]
  > {
    const label = crypto.randomUUID();
    const channel = this._peerConnection.createDataChannel(label, {
      ordered: true,
    });
    while (this._signalingLock) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    this._signalingLock = true;
    const offer = await this._peerConnection.createOffer(offerOptions);
    await this._peerConnection.setLocalDescription(offer);

    return [channel, offer];
  }

  public close() {
    this._peerConnection.close();
  }

  private waitForIceGatheringComplete(peerConnection: RTCPeerConnection) {
    return new Promise((resolve) => {
      if (peerConnection.iceGatheringState === "complete") {
        resolve(true);
      } else {
        const checkState = () => {
          if (peerConnection.iceGatheringState === "complete") {
            peerConnection.removeEventListener(
              "icegatheringstatechange",
              checkState,
            );
            resolve(true);
          }
        };
        peerConnection.addEventListener("icegatheringstatechange", checkState);
      }
    });
  }
}

export function adjustExtmap(
  sdp: RTCSessionDescriptionInit,
): RTCSessionDescriptionInit {
  if (!sdp.sdp) {
    return sdp;
  }
  const res = sdpTransform.parse(sdp.sdp);

  const media = res.media.map((media) => {
    const extmap = media.ext?.map(({ value: index, uri: uri }) => {
      const order = findExtmapOrder(uri);
      if (order) {
        return { value: order, uri: uri };
      } else {
        return { value: index, uri: uri };
      }
    });

    media.ext = extmap;
    return media;
  });
  res.media = media;

  const str = sdpTransform.write(res);
  const newSdp = new RTCSessionDescription({ type: sdp.type, sdp: str });
  return newSdp;
}
