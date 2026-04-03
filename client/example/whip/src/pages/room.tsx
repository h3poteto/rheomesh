import { useRouter } from "next/router";
import { useEffect, useRef, useState } from "react";
import {
  PublishTransport,
  SubscribeTransport,
  simulcastEncodings,
} from "rheomesh";
import { rfc8840Candidate } from "../../../../rheomesh/lib/publishTransport";

const peerConnectionConfig: RTCConfiguration = {
  iceServers: [{ urls: "stun:stun.l.google.com:19302" }],
};

export default function Room() {
  const router = useRouter();
  const host = `http://localhost:${process.env.NEXT_PUBLIC_SERVER_PORT || "4000"}`;

  const [room, setRoom] = useState("");
  const [localVideo, setLocalVideo] = useState<MediaStream | null>(null);
  const [recevingVideo, setRecevingVideo] = useState<{
    [publisherId: string]: MediaStream;
  }>({});
  const [recevingAudio, setRecevingAudio] = useState<{
    [publisherId: string]: MediaStream;
  }>({});
  const [subscriberIds, setSubscriberIds] = useState<Array<string>>([]);
  const [connected, setConnected] = useState(false);

  const sendingVideoRef = useRef<HTMLVideoElement>(null);
  const publishTransport = useRef<PublishTransport>(null);
  const sessionId = useRef<string | null>(null);
  const etag = useRef<string | null>(null);

  const ws = useRef<WebSocket | null>(null);
  const subscribeTransport = useRef<SubscribeTransport | null>(null);

  useEffect(() => {
    if (router.query.room) {
      setRoom(router.query.room as string);

      (async () => {
        const response = await fetch(
          `${host}/rooms/${router.query.room}/join`,
          { method: "POST" },
        );
        console.debug("Join room response:", response);
        if (response.ok) {
          const json = await response.json();
          sessionId.current = json.session_id;
          startPublishPeer();
        }
      })();
    }
  }, [router.query.room]);

  /** publisher with WHIP **/
  const startPublishPeer = () => {
    if (!publishTransport.current) {
      publishTransport.current = new PublishTransport(
        peerConnectionConfig,
        true,
      );
      publishTransport.current.on(
        "icecandidate",
        (candidate: RTCIceCandidate) => {
          if (sessionId.current && etag.current) {
            const fragment = rfc8840Candidate(candidate);
            fetch(`${host}/whip/${sessionId.current}`, {
              method: "PATCH",
              body: fragment,
              headers: {
                "Content-Type": "application/trickle-ice-sdpfrag",
                "If-Match": etag.current!,
              },
            }).then((response) => {
              if (response.ok && response.status === 204) {
                console.debug("ICE candidate sent successfully");
              } else if (response.ok) {
                console.warn("Unexpected response status:", response.status);
              } else {
                console.error(
                  "Failed to send ICE candidate:",
                  response.statusText,
                );
              }
            });
          }
        },
      );
    }
  };

  const capture = async () => {
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: true,
      audio: false,
    });

    if (sendingVideoRef.current) {
      sendingVideoRef.current.srcObject = stream;
    }

    await publish(stream);
    setLocalVideo(stream);
  };

  const publish = async (stream: MediaStream) => {
    stream.getTracks().forEach(async (track) => {
      const publisher = await publishTransport.current!.publish(track, {
        encodings: simulcastEncodings(),
      });
      const response = await fetch(`${host}/whip/${sessionId.current}`, {
        method: "POST",
        body: publisher.offer.sdp,
        headers: { "Content-Type": "application/sdp" },
      });
      if (!response.ok) {
        console.error("Failed to publish track:", response.statusText);
        return;
      }
      etag.current = response.headers.get("Etag");
      console.debug("Received Etag:", etag.current);
      console.debug("WHIP response:", response);
      const sdp = new RTCSessionDescription({
        type: "answer",
        sdp: await response.text(),
      });
      await publishTransport.current!.setAnswer(sdp);
    });
  };

  /** subscriber with WebSocket **/
  const connect = () => {
    close();
    ws.current = new WebSocket(
      `ws://localhost:${process.env.NEXT_PUBLIC_SERVER_PORT || "4000"}/socket?room=${room}`,
    );
    ws.current.onopen = () => {
      console.debug("Connected websocket server");
      startSubscribePeer();
      setConnected(true);
    };
    ws.current.onclose = () => {
      console.debug("Disconnected from websocket server");
      setConnected(false);
    };
    ws.current.onerror = (e) => {
      console.error(e);
    };
    ws.current.onmessage = messageHandler;
    setInterval(() => {
      if (ws.current && ws.current.readyState === WebSocket.OPEN) {
        ws.current.send(JSON.stringify({ action: "Ping" }));
      }
    }, 5000);
  };

  const close = () => {
    subscriberIds.forEach((id) => {
      ws.current!.send(
        JSON.stringify({
          action: "StopSubscribe",
          subscriberId: id,
        }),
      );
    });
    setSubscriberIds([]);
    subscribeTransport.current?.close();
    ws.current?.close();
    ws.current = null;
    setConnected(false);
  };

  const startSubscribePeer = () => {
    if (!subscribeTransport.current) {
      subscribeTransport.current = new SubscribeTransport(peerConnectionConfig);
      ws.current!.send(JSON.stringify({ action: "SubscriberInit" }));
      subscribeTransport.current.on("icecandidate", (candidate) => {
        ws.current!.send(
          JSON.stringify({
            action: "SubscriberIce",
            candidate: candidate,
          }),
        );
      });
    }
  };

  const messageHandler = (event: MessageEvent) => {
    console.debug("Received message: ", event.data);
    const message = JSON.parse(event.data);
    switch (message.action) {
      case "Offer":
        subscribeTransport.current!.setOffer(message.sdp).then((answer) => {
          ws.current!.send(JSON.stringify({ action: "Answer", sdp: answer }));
        });
        break;
      case "SubscriberIce":
        subscribeTransport.current!.addIceCandidate(message.candidate);
        break;
      case "Published":
        message.publisherIds.forEach((publisherId: string) => {
          ws.current!.send(
            JSON.stringify({
              action: "Subscribe",
              publisherId: publisherId,
            }),
          );
          subscribeTransport
            .current!.subscribe(publisherId)
            .then((subscriber) => {
              const stream = new MediaStream([subscriber.track]);
              if (subscriber.track.kind === "audio") {
                setRecevingAudio((prev) => ({
                  ...prev,
                  [publisherId]: stream,
                }));
              } else {
                setRecevingVideo((prev) => ({
                  ...prev,
                  [publisherId]: stream,
                }));
              }
            });
        });

        break;
      case "Subscribed":
        setSubscriberIds((prev) => [...prev, message.subscriberId]);
        break;
      case "Pong":
        console.debug("pong");
        break;
      default:
        console.error("Unknown message type: ", message);
        break;
    }
  };

  return (
    <div>
      <h1>Room: {room}</h1>
      <div>
        <button id="capture" onClick={capture} disabled={localVideo !== null}>
          Capture
        </button>
      </div>
      <h3>Sending Video</h3>
      <video
        autoPlay
        muted
        id="sending-video"
        ref={sendingVideoRef}
        width={480}
      ></video>
      <h3>Receving</h3>
      <button id="connect" onClick={connect} disabled={connected}>
        Connect
      </button>
      {Object.keys(recevingVideo).map((key) => (
        <div key={key}>
          {recevingVideo[key] && (
            <video
              id={key}
              muted
              className="receiving-video"
              autoPlay
              ref={(video) => {
                if (video && recevingVideo[key]) {
                  video.srcObject = recevingVideo[key];
                } else {
                  console.warn("video element or track is null");
                }
              }}
              width={480}
            ></video>
          )}
        </div>
      ))}
      {Object.keys(recevingAudio).map((key) => (
        <div key={key}>
          {recevingAudio[key] && (
            <audio
              id={key}
              autoPlay
              controls
              ref={(audio) => {
                if (audio && recevingAudio[key]) {
                  audio.srcObject = recevingAudio[key];
                } else {
                  console.warn("audio element or track is null");
                }
              }}
            ></audio>
          )}
        </div>
      ))}
    </div>
  );
}
