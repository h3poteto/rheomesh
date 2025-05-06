export type Publisher = {
  id: string;
  offer: RTCSessionDescriptionInit;
};

export type DataPublisher = {
  id: string;
  channel: RTCDataChannel;
  offer: RTCSessionDescriptionInit;
};
