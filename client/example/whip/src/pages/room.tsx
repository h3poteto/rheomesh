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

  const sendingVideoRef = useRef<HTMLVideoElement>(null);
  const publishTransport = useRef<PublishTransport>(null);
  const sessionId = useRef<string | null>(null);
  const etag = useRef<string | null>(null);

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
    </div>
  );
}
