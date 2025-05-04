import { adjustExtmap, findTrackId } from "../src/publishTransport";
import fs from "fs";
import path from "path";
import * as sdpTransform from "sdp-transform";

describe("adjustExtmap", () => {
  describe("single video sdp", () => {
    it("extmap should be remapped", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_video_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const res = adjustExtmap(originalSdp);
      const correctPath = path.join(__dirname, "sdp/sdp_video_correct");
      const correctContent = fs.readFileSync(correctPath, "utf-8");

      // Compare extmap
      const adjusted = sdpTransform.parse(res.sdp as string);
      const correct = sdpTransform.parse(correctContent);
      expect(adjusted).toEqual(correct);
    });
  });
  describe("single audio sdp", () => {
    it("extmap should be remapped", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_audio_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const res = adjustExtmap(originalSdp);
      const correctPath = path.join(__dirname, "sdp/sdp_audio_correct");
      const correctContent = fs.readFileSync(correctPath, "utf-8");

      // Compare extmap
      const adjusted = sdpTransform.parse(res.sdp as string);
      const correct = sdpTransform.parse(correctContent);
      expect(adjusted).toEqual(correct);
    });
  });
  describe("video and audio sdp", () => {
    it("extmap should be remapped", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_video_audio_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const res = adjustExtmap(originalSdp);
      const correctPath = path.join(__dirname, "sdp/sdp_video_audio_correct");
      const correctContent = fs.readFileSync(correctPath, "utf-8");

      // Compare extmap
      const adjusted = sdpTransform.parse(res.sdp as string);
      const correct = sdpTransform.parse(correctContent);
      expect(adjusted).toEqual(correct);
    });
  });
  describe("audio and video sdp", () => {
    it("extmap should be remapped", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_audio_video_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const res = adjustExtmap(originalSdp);
      const correctPath = path.join(__dirname, "sdp/sdp_audio_video_correct");
      const correctContent = fs.readFileSync(correctPath, "utf-8");

      // Compare extmap
      const adjusted = sdpTransform.parse(res.sdp as string);
      const correct = sdpTransform.parse(correctContent);
      expect(adjusted).toEqual(correct);
    });
  });
});

describe("findTrackId", () => {
  describe("single video sdp", () => {
    it("should find track id", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_video_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const trackId = findTrackId(originalSdp, "0");
      expect(trackId).toBe("b0734cb9-da91-4957-8957-25de07ab05d0");
    });
  });
  describe("single audio sdp", () => {
    it("should find track id", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_audio_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const trackId = findTrackId(originalSdp, "0");
      expect(trackId).toBe("6307ea36-e89b-4739-9cf3-81a489258d19");
    });
  });
  describe("video and audio sdp", () => {
    it("should find track id", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_video_audio_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const trackId = findTrackId(originalSdp, "1");
      expect(trackId).toBe("9094aca0-c591-438e-9868-fde45869debc");
    });
  });
  describe("audio and video sdp", () => {
    it("should find track id", () => {
      const originalPath = path.join(__dirname, "sdp/sdp_audio_video_original");
      const originalContent = fs.readFileSync(originalPath, "utf-8");
      const originalSdp = new RTCSessionDescription({
        type: "offer",
        sdp: originalContent,
      });
      const trackId = findTrackId(originalSdp, "0");
      expect(trackId).toBe("6307ea36-e89b-4739-9cf3-81a489258d19");
    });
  });
});
