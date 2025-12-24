use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

pub fn parse_ice_ufrag(input: &str) -> Option<&str> {
    input
        .lines()
        .find_map(|line| line.strip_prefix("a=ice-ufrag:"))
}

pub fn parse_ice_pwd(input: &str) -> Option<&str> {
    input
        .lines()
        .find_map(|line| line.strip_prefix("a=ice-pwd:"))
}

fn parse_media_blocks(input: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current_mid: Option<String> = None;
    let mut current_content = String::new();

    for line in input.lines() {
        if let Some(mid) = line.strip_prefix("a=mid:") {
            current_mid = Some(mid.to_string());
            current_content.clear();
        } else if let Some(mid) = current_mid.clone() {
            current_content.push_str(line);
            current_content.push('\n');

            if line == "a=end-of-candidates" {
                blocks.push((mid, current_content.clone()));
                current_content.clear();
            }
        }
    }
    blocks
}

pub fn parse_candidates(input: &str) -> Vec<RTCIceCandidateInit> {
    let mut ice_candidates = Vec::new();
    let media_blocks = parse_media_blocks(input);

    let ice_ufrag = parse_ice_ufrag(input);

    for (mid, media) in media_blocks {
        for line in media.lines() {
            if line.starts_with("a=candidate:") {
                let candidate_str = line.strip_prefix("a=").unwrap();
                let candidate = RTCIceCandidateInit {
                    candidate: candidate_str.to_string(),
                    sdp_mid: Some(mid.clone()),
                    sdp_mline_index: None,
                    username_fragment: ice_ufrag.map(|s| s.to_string()),
                };
                ice_candidates.push(candidate);
            }
        }
    }
    ice_candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ice_ufrag() {
        // Example SDP from https://datatracker.ietf.org/doc/html/draft-ietf-wish-whip#section-4.3.2
        let sdp = "
a=group:BUNDLE 0 1
m=audio 9 UDP/TLS/RTP/SAVPF 111
a=mid:0
a=ice-ufrag:EsAw
a=ice-pwd:P2uYro0UCOQ4zxjKXaWCBui1
a=candidate:1387637174 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag EsAw network-id 1
a=candidate:3471623853 1 udp 2122194687 198.51.100.2 61765 typ host generation 0 ufrag EsAw network-id 2
a=candidate:473322822 1 tcp 1518280447 192.0.2.1 9 typ host tcptype active generation 0 ufrag EsAw network-id 1
a=candidate:2154773085 1 tcp 1518214911 198.51.100.2 9 typ host tcptype active generation 0 ufrag EsAw network-id 2
a=end-of-candidates
";
        let result = parse_ice_ufrag(sdp);
        println!("{:?}", result);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "EsAw");
    }

    #[test]
    fn test_parse_candidate_sdp_multiple() {
        // Example SDP from https://www.rfc-editor.org/rfc/rfc8840
        let sdp = "
a=ice-pwd:asd88fgpdd777uzjYhagZg
a=ice-ufrag:8hhY
m=audio 9 RTP/AVP 0
a=mid:1
a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 5000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 5001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 5010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 5011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 5010 typ srflx raddr 192.0.2.1 rport 8998
a=candidate:2 2 UDP 1694498815 192.0.2.3 5011 typ srflx raddr 192.0.2.1 rport 8998
a=end-of-candidates
m=audio 9 RTP/AVP 0
a=mid:2
a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 6000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 6001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 6010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 6011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 6010 typ srflx raddr 192.0.2.1 rport 9998
a=candidate:2 2 UDP 1694498815 192.0.2.3 6011 typ srflx raddr 192.0.2.1 rport 9998
a=end-of-candidates
";
        let result = parse_ice_ufrag(sdp);
        println!("{:?}", result);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "8hhY");
    }

    #[test]
    fn test_parse_media_blocks() {
        let sdp = "
a=ice-pwd:asd88fgpdd777uzjYhagZg
a=ice-ufrag:8hhY
m=audio 9 RTP/AVP 0
a=mid:1
a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 5000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 5001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 5010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 5011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 5010 typ srflx raddr 192.0.2.1 rport 8998
a=candidate:2 2 UDP 1694498815 192.0.2.3 5011 typ srflx raddr 192.0.2.1 rport 8998
a=end-of-candidates
m=audio 9 RTP/AVP 0
a=mid:2
a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 6000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 6001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 6010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 6011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 6010 typ srflx raddr 192.0.2.1 rport 9998
a=candidate:2 2 UDP 1694498815 192.0.2.3 6011 typ srflx raddr 192.0.2.1 rport 9998
a=end-of-candidates
";
        let blocks = parse_media_blocks(sdp);
        println!("{:?}", blocks);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].0, "1");
        assert_eq!(
            blocks[0].1,
            String::from(
                "a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 5000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 5001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 5010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 5011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 5010 typ srflx raddr 192.0.2.1 rport 8998
a=candidate:2 2 UDP 1694498815 192.0.2.3 5011 typ srflx raddr 192.0.2.1 rport 8998
a=end-of-candidates
"
            )
        );
        assert_eq!(blocks[1].0, "2");
        assert_eq!(
            blocks[1].1,
            String::from(
                "a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 6000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 6001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 6010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 6011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 6010 typ srflx raddr 192.0.2.1 rport 9998
a=candidate:2 2 UDP 1694498815 192.0.2.3 6011 typ srflx raddr 192.0.2.1 rport 9998
a=end-of-candidates
"
            )
        );
    }

    #[test]
    fn test_parse_candidates() {
        let sdp = "
a=group:BUNDLE 0 1
m=audio 9 UDP/TLS/RTP/SAVPF 111
a=mid:0
a=ice-ufrag:EsAw
a=ice-pwd:P2uYro0UCOQ4zxjKXaWCBui1
a=candidate:1387637174 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag EsAw network-id 1
a=candidate:3471623853 1 udp 2122194687 198.51.100.2 61765 typ host generation 0 ufrag EsAw network-id 2
a=candidate:473322822 1 tcp 1518280447 192.0.2.1 9 typ host tcptype active generation 0 ufrag EsAw network-id 1
a=candidate:2154773085 1 tcp 1518214911 198.51.100.2 9 typ host tcptype active generation 0 ufrag EsAw network-id 2
a=end-of-candidates
";
        let candidates = parse_candidates(sdp);
        println!("{:#?}", candidates);
        assert_eq!(candidates.len(), 4);
        assert_eq!(
            candidates[0].candidate,
            String::from(
                "candidate:1387637174 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag EsAw network-id 1"
            )
        );
        assert_eq!(candidates[0].sdp_mid.as_deref(), Some("0"));
        assert_eq!(candidates[0].username_fragment.as_deref(), Some("EsAw"));
    }

    #[test]
    fn test_parse_candidates_multiple() {
        let sdp = "
a=ice-pwd:asd88fgpdd777uzjYhagZg
a=ice-ufrag:8hhY
m=audio 9 RTP/AVP 0
a=mid:1
a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 5000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 5001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 5010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 5011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 5010 typ srflx raddr 192.0.2.1 rport 8998
a=candidate:2 2 UDP 1694498815 192.0.2.3 5011 typ srflx raddr 192.0.2.1 rport 8998
a=end-of-candidates
m=audio 9 RTP/AVP 0
a=mid:2
a=candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 6000 typ host
a=candidate:1 2 UDP 2130706432 2001:db8:a0b:12f0::1 6001 typ host
a=candidate:1 1 UDP 2130706431 192.0.2.1 6010 typ host
a=candidate:1 2 UDP 2130706431 192.0.2.1 6011 typ host
a=candidate:2 1 UDP 1694498815 192.0.2.3 6010 typ srflx raddr 192.0.2.1 rport 9998
a=candidate:2 2 UDP 1694498815 192.0.2.3 6011 typ srflx raddr 192.0.2.1 rport 9998
a=end-of-candidates
";
        let candidates = parse_candidates(sdp);
        println!("{:#?}", candidates);
        assert_eq!(candidates.len(), 12);
        assert_eq!(
            candidates[6].candidate,
            String::from("candidate:1 1 UDP 2130706432 2001:db8:a0b:12f0::1 6000 typ host")
        );
        assert_eq!(candidates[0].sdp_mid.as_deref(), Some("1"));
        assert_eq!(candidates[0].username_fragment.as_deref(), Some("8hhY"));
        assert_eq!(candidates[11].sdp_mid.as_deref(), Some("2"));
        assert_eq!(candidates[11].username_fragment.as_deref(), Some("8hhY"));
    }
}
