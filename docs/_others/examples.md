---
layout: post
title: Examples
---
We provide real SFU server examples. Please clone the [repository](https://github.com/h3poteto/rheomesh) and try the examples.

```bash
$ git clone https://github.com/h3poteto/rheomesh.git
```


# Data channels
Launch server side.
```bash
$ cd sfu
$ cargo run --example data_server
```

Launch client side.
```bash
$ cd client
$ yarn install
$ yarn workspace rheomesh build
$ yarn workspace data dev
```

You can access the frontend service with `localhost:5173`.
Then, click `connect` on browser 1 and browser 2. And click `publish` on the browser 1. Last, type some words in the form, and click `send` button on the browser 1. As a result, you can see the words on the browser 2.

# Camera
Launch server side.
```bash
$ cd sfu
$ cargo run --example media_server
```

Launch client side.
```bash
$ cd client
$ yarn install
$ yarn workspace rheomesh build
$ yarn workspace camera dev
```

You can access the frontend service with `localhost:3000`, and please join an arbitrary room, e.g. `my-room`.
Then, click `connect` and `camera` on browser 1. Next, click `connect` on browser 2. As a result, you can see the camera video on the browser 2.

# Screen sharing
Launch server side.
```bash
$ cd sfu
$ cargo run --example media_server
```

Launch client side.
```bash
$ cd client
$ yarn install
$ yarn workspace rheomesh build
$ yarn workspace multiple dev
```

You can access the frontend service with `localhost:3000`, and please join an arbitrary room, e.g. `my-room`.

# Relay with two servers
To use relay feature you need two or more servers. In this section, we are using docker command to simulate two servers.
**Note**: Please update `PUBLIC_IP` environment variable in `compose.yaml` file.

```bash
$ docker compose up -d
```

When you access `localhost:3001`, the frontend connects server 1 (`sfu1`). When you access `localhost:3002`, the frontend connects server 2 (`sfu2`). Please try:

1. Join the same room, e.g. `my-room` on `localhost:3001` and `localhost:3002`
1. `Connect` on `localhost:3001`
1. `Capture` a screen on `localhost:3001`
1. `Connect` on `localhost:3002`
1. You can see the screen on `localhost:3002`

# Recording
Rheomesh provides raw RTP forwarding capabilities designed for recording with ffmpeg or GStreamer. This allows you to receive RTP packets and record them directly. Let's try this with an example.

Launch SFU server.
```bash
$ cd sfu
$ cargo run --example media_server
```

Launch client side.

```bash
$ cd client
$ yarn install
$ yarn workspace rheomesh build
$ yarn workspace camera dev
```

You can access the frontend service with `localhost:3000`, and please join an arbitrary room, e.g. `my-room`.
Then, click `connect` and `camera` on your browser. Next, you can obtain SDP for recording with clicking `Get Recording SDP`. Please write it to `stream.sdp` file.

After you start ffmpeg or GStreamer process with the SDP file, click `Start Recording` button. Then, recording will be started.

## Using ffmpeg
For playback:
```bash
$ ffplay -protocol_whitelist file,rtp,udp -analyzeduration 10000000 -probesize 50000000 -f sdp -i stream.sdp
```

For recording file:
```bash
$ ffmpeg -protocol_whitelist file,rtp,udp -i stream.sdp -c copy output.mkv
```


## Using GStreamer
For playback:

```bash
$ gst-launch-1.0 udpsrc port=30001 ! \
  application/x-rtp,payload=103,encoding-name=H264 ! \
  rtph264depay ! \
  h264parse ! \
  avdec_h264 ! \
  videoconvert ! \
  autovideosink
```

For recording file:
```bash
$ gst-launch-1.0 udpsrc port=30001 ! \
  application/x-rtp,payload=103,encoding-name=H264 ! \
  rtph264depay ! \
  h264parse ! \
  avdec_h264 ! \
  matroskamux ! \
  filesink location=output.mkv
```
