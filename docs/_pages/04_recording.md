---
title: Recording
layout: post
---

Rheomesh can export raw RTP packets to record audio and video using GStreamer or ffmpeg. Rheomesh provides two useful methods for recording.

- `generate_sdp` can generate a simple SDP to receive RTP packets.
- `start_recording` starts transporting RTP packets to an endpoint.


# Prepare Transport
At first, please prepare [RecordingTransport](https://docs.rs/rheomesh/latest/rheomesh/recording/recording_transport/struct.RecordingTransport.html).

```rust
let recording_transport = router
  .create_recording_transport(recording_ip, recording_port)
  .await
  .expect("Failed to create recording_transport");
```


# Generate SDP
```rust
let sdp = recording_transport
  .generate_sdp(publisher_id)
  .await
  .expect("Failed to generate SDP");
```

The `publisher_id` can be obtained from the Publisher object. See [Getting Started]({{ site.baseurl }}/pages/02_getting_started/#server-side-2) for details.

You can use this SDP with both ffmpeg and GStreamer. For example, write SDP to `stream.sdp` file.

## Using ffmpeg
For playback:
```bash
# -protocol_whitelist: Allow file, rtp, udp protocols
# -analyzeduration: Time to analyze input stream (10 seconds)
# -probesize: Input probing size (50MB)
$ ffplay -protocol_whitelist file,rtp,udp -analyzeduration 10000000 -probesize 50000000 -f sdp -i stream.sdp
```

For recording to file:
```bash
$ ffmpeg -protocol_whitelist file,rtp,udp -i stream.sdp -c copy output.mkv
```

## Using GStreamer
For playback:
```bash
$ gst-launch-1.0 filesrc location=stream.sdp ! sdpdemux ! decodebin ! autovideosink
```

For recording to file:
```bash
$ gst-launch-1.0 filesrc location=stream.sdp ! sdpdemux ! matroskamux ! filesink location=output.mkv
```

# Start transporting packets
```rust
let track = recording_transport.start_recording(publisher_id).await.expect("Failed to start recording");
```
