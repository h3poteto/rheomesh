import Head from "next/head";
import { useRouter } from "next/router";
import { useState } from "react";

export default function Home() {
  const [room, setRoom] = useState("");

  const router = useRouter();

  const openRoom = () => {
    router.push(`/room?room=${room}`);
  };

  return (
    <>
      <Head>
        <title>Multiple</title>
        <meta name="description" content="Multiple media example" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <link rel="icon" href="/favicon.ico" />
      </Head>
      <div>
        <main>
          <div>
            <input
              placeholder="The room name"
              value={room}
              onChange={(v) => setRoom(v.target.value)}
            />
            <button onClick={openRoom}>Join</button>
          </div>
        </main>
      </div>
    </>
  );
}
