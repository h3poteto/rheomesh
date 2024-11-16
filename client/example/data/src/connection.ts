import { PublishTransport, SubscribeTransport } from "rheomesh";

let publishTransport: PublishTransport;
let subscribeTransport: SubscribeTransport;
let inputText: HTMLInputElement;
let remoteText: HTMLSpanElement;
let subscriberId: string;

let connectButton: HTMLButtonElement;
let startButton: HTMLButtonElement;
let sendButton: HTMLButtonElement;
let stopButton: HTMLButtonElement;

let publishedChannel: RTCDataChannel;

let ws: WebSocket;

const peerConnectionConfig: RTCConfiguration = {
  iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
};

export function setup() {
  connectButton = document.getElementById("connect") as HTMLButtonElement;
  startButton = document.getElementById("start") as HTMLButtonElement;
  sendButton = document.getElementById("send") as HTMLButtonElement;
  stopButton = document.getElementById("stop") as HTMLButtonElement;
  inputText = document.getElementById("data") as HTMLInputElement;
  remoteText = document.getElementById("remote_data") as HTMLSpanElement;
  sendButton.disabled = true;
  stopButton.disabled = true;
  startButton.addEventListener("click", start);
  sendButton.addEventListener("click", send);
  connectButton.addEventListener("click", connect);
  stopButton.addEventListener("click", stop);
}

async function connect() {
  console.log("Starting connection");
  ws = new WebSocket("ws://localhost:4000/socket?room=example");
  ws.onopen = () => {
    console.log("Connected to server");
    connectButton.disabled = true;
    sendButton.disabled = false;
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

async function start() {
  startButton.disabled = true;
  stopButton.disabled = false;
  ws.send(JSON.stringify({ action: "RequestPublish" }));
}

async function send() {
  if (inputText.value) {
    publishedChannel.send(inputText.value);
  } else {
    console.error("There is no input text");
  }
}

async function stop() {
  console.log("Stopping");
  ws.send(
    JSON.stringify({ action: "StopPublish", publisherId: publishedChannel.id }),
  );
  if (subscriberId) {
    ws.send(
      JSON.stringify({
        action: "StopSubscribe",
        subscriberId: subscriberId,
      }),
    );
  }
  startButton.disabled = false;
  stopButton.disabled = true;
  sendButton.disabled = true;
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

async function publish() {
  const [channel, offer] = await publishTransport.publishData();
  publishedChannel = channel;
  ws.send(
    JSON.stringify({
      action: "Offer",
      sdp: offer,
    }),
  );
  channel.onopen = (_ev) => {
    ws.send(JSON.stringify({ action: "Publish", label: channel.label }));
  };
}

function messageHandler(event: MessageEvent) {
  console.debug("Received message: ", event.data);
  const message = JSON.parse(event.data);
  switch (message.action) {
    case "StartAsPublisher":
      publish();
      break;
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
      ws.send(
        JSON.stringify({
          action: "Subscribe",
          publisherId: message.publisherId,
        }),
      );
      subscribeTransport.subscribeData(message.publisherId).then((channel) => {
        channel.onmessage = ondata;
      });
      break;
    case "Subscribed":
      subscriberId = message.subscriberId;
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

function ondata(ev: MessageEvent) {
  console.log("ondata", ev);
  const text = new TextDecoder().decode(ev.data);
  remoteText.innerText = text;
}
