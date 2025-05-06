export type Subscriber = {
  publisherId: string;
  track: MediaStreamTrack;
};

export type DataSubscriber = {
  publisherId: string;
  channel: RTCDataChannel;
};
