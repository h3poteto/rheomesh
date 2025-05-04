import { PublishTransport, SubscribeTransport } from "rheomesh";

let publishTransport: PublishTransport;
let subscribeTransport: SubscribeTransport;

let localVideo: HTMLVideoElement;
let remoteVideo: HTMLVideoElement;
let remoteAudio: HTMLAudioElement;
let localStreams: Array<MediaStream> = [];
let subscriberIds: Array<string> = [];
let publisherIds: Array<string> = [];

let connectButton: HTMLButtonElement;
let captureButton: HTMLButtonElement;
let micButton: HTMLButtonElement;
let stopButton: HTMLButtonElement;

let ws: WebSocket;

const peerConnectionConfig: RTCConfiguration = {
  iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
};

export function setup() {
  localVideo = document.getElementById("localVideo") as HTMLVideoElement;
  remoteVideo = document.getElementById("remoteVideo") as HTMLVideoElement;
  remoteAudio = document.getElementById("remoteAudio") as HTMLAudioElement;

  connectButton = document.getElementById("connect") as HTMLButtonElement;
  captureButton = document.getElementById("capture") as HTMLButtonElement;
  micButton = document.getElementById("mic") as HTMLButtonElement;
  stopButton = document.getElementById("stop") as HTMLButtonElement;
  captureButton.disabled = true;
  micButton.disabled = true;
  stopButton.disabled = true;
  captureButton.addEventListener("click", capture);
  micButton.addEventListener("click", mic);
  connectButton.addEventListener("click", connect);
  stopButton.addEventListener("click", stop);
}

async function connect() {
  console.log("Starting connection");
  ws = new WebSocket("ws://localhost:4000/socket?room=example");
  ws.onopen = () => {
    console.log("Connected to server");
    connectButton.disabled = true;
    captureButton.disabled = false;
    micButton.disabled = false;
    stopButton.disabled = false;
    startPublishPeer();
    startSubscribePeer();
  };
  ws.close = () => {
    console.log("Disconnected from server");
  };
  ws.onmessage = messageHandler;
  setInterval(() => {
    if (ws.readyState === ws.OPEN) {
      ws.send(JSON.stringify({ action: "Ping" }));
    }
  }, 5000);
}

async function capture() {
  console.log("Requesting local stream");
  captureButton.disabled = true;
  stopButton.disabled = false;
  try {
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: true,
      audio: true,
    });
    console.log("Received local stream");
    localVideo.srcObject = stream;
    localStreams.push(stream);
    await publish(stream);
  } catch (e) {
    console.error("Error accessing media devices.", e);
  }
}

async function mic() {
  micButton.disabled = true;
  stopButton.disabled = false;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({
      video: false,
      audio: true,
    });
    localStreams.push(stream);
    await publish(stream);
  } catch (e) {
    console.error("Error accessing mic", e);
  }
}

async function stop() {
  console.log("Stopping");
  publisherIds.forEach((id) => {
    ws.send(JSON.stringify({ action: "StopPublish", publisherId: id }));
  });
  localStreams.forEach((stream) => {
    stream.getTracks().forEach((track) => {
      track.stop();
    });
  });

  subscriberIds.forEach((id) => {
    ws.send(
      JSON.stringify({
        action: "StopSubscribe",
        subscriberId: id,
      }),
    );
  });
  localStreams = [];
  subscriberIds = [];
  localVideo.srcObject = null;
  stopButton.disabled = true;
  captureButton.disabled = false;
  micButton.disabled = false;
  connectButton.disabled = true;
}

function startPublishPeer() {
  if (!publishTransport) {
    publishTransport = new PublishTransport(peerConnectionConfig);
    ws.send(JSON.stringify({ action: "PublisherInit" }));
    publishTransport.on("icecandidate", (candidate) => {
      ws.send(
        JSON.stringify({
          action: "PublisherIce",
          candidate: candidate,
        }),
      );
    });
    publishTransport.on("negotiationneeded", (offer) => {
      ws.send(
        JSON.stringify({
          action: "Offer",
          sdp: offer,
        }),
      );
    });
  }
}

function startSubscribePeer() {
  if (!subscribeTransport) {
    subscribeTransport = new SubscribeTransport(peerConnectionConfig);
    ws.send(JSON.stringify({ action: "SubscriberInit" }));
    subscribeTransport.on("icecandidate", (candidate) => {
      ws.send(
        JSON.stringify({
          action: "SubscriberIce",
          candidate: candidate,
        }),
      );
    });
  }
}

async function publish(stream: MediaStream) {
  stream.getTracks().forEach(async (track) => {
    const publisher = await publishTransport.publish(track);
    ws.send(
      JSON.stringify({
        action: "Offer",
        sdp: publisher.offer,
      }),
    );
    ws.send(JSON.stringify({ action: "Publish", publisherId: publisher.id }));
    publisherIds.push(publisher.id);
  });
}

function messageHandler(event: MessageEvent) {
  console.debug("Received message: ", event.data);
  const message = JSON.parse(event.data);
  switch (message.action) {
    case "Offer":
      subscribeTransport.setOffer(message.sdp).then((answer) => {
        ws.send(JSON.stringify({ action: "Answer", sdp: answer }));
      });
      break;
    case "Answer":
      publishTransport.setAnswer(message.sdp);
      break;
    case "SubscriberIce":
      subscribeTransport.addIceCandidate(message.candidate);
      break;
    case "PublisherIce":
      publishTransport.addIceCandidate(message.candidate);
      break;
    case "Published":
      message.publisherIds.forEach((publisherId: string) => {
        ws.send(
          JSON.stringify({
            action: "Subscribe",
            publisherId: publisherId,
          }),
        );
        subscribeTransport.subscribe(publisherId).then((subscriber) => {
          const stream = new MediaStream([subscriber.track]);
          if (subscriber.track.kind === "audio") {
            remoteAudio.srcObject = stream;
          } else {
            remoteVideo.srcObject = stream;
          }
        });
      });

      break;
    case "Subscribed":
      subscriberIds.push(message.subscriberId);
      stopButton.disabled = false;
      break;
    case "Pong":
      console.debug("pong");
      break;
    default:
      console.error("Unknown message type: ", message);
      break;
  }
}
