use super::*;
use wire::*;
#[test]
fn voice_dump_short_heard_audio_reaches_wav() {
    let path = std::env::temp_dir().join(format!("voice-dump-test-{}.wav", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    use std::io::Write;
    (&file).write_all(&[0; 44]).unwrap();
    let now = Instant::now();
    let mut dump = Dump {
        file: Some(file),
        start: now,
        last: now,
        base: 0,
        samples: Vec::new(),
        ends: HashMap::new(),
    };
    let mut c = Channel {
        reorder: Reorder::default(),
        decoder: OpusDecoder::new(48000, 1).unwrap(),
        last: sound(0),
        arrival: now,
    };
    let frames = messages::encode_audio(&vec![0.2; 960], 4500).unwrap();
    let mut s = sound(0);
    s.opus = frames[0].clone();
    let (tx, rx) = mpsc::sync_channel(2);
    decode(&mut c, vec![Some(s)], &tx, &mut HashMap::new(), &mut dump);
    assert!(matches!(rx.try_recv(), Ok(Event::Pcm(_, _))));
    dump.start = now - Duration::from_secs(2);
    dump.last = now - Duration::from_secs(2);
    dump.flush(false);
    let pcm = sender::read_wav(&path).unwrap();
    assert!(pcm.len() >= 960);
    assert!(pcm.iter().any(|v| v.abs() > 0.01));
    drop(dump);
    std::fs::remove_file(path).unwrap();
}
fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
#[test]
fn gcm_spec_vector() {
    let key = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
    let iv = hex("a0a1a2a3a4a5a6a7a8a9aaab").try_into().unwrap();
    let plain = hex("0868656c6c6f20766f696365");
    let c = Crypto::new(&key);
    let sealed = c.seal(iv, &plain).unwrap();
    assert_eq!(
        &sealed[12..],
        hex("a2ee5dd712e6137ce511d66537b6213215efcd1616a3eecb63bea4fc")
    );
    assert_eq!(c.open(&sealed), Some(plain));
    let mut bad = sealed;
    bad[15] ^= 1;
    assert!(c.open(&bad).is_none());
}
fn sound(sequence: i64) -> Sound {
    Sound {
        channel: 1,
        sender: 2,
        placement: Placement::Player,
        opus: vec![3, 4],
        sequence,
        distance: 48.0,
        flags: 3,
        category: Some("speech".into()),
    }
}
#[test]
fn every_packet_layout() {
    let packets = vec![
        Packet::Mic {
            opus: vec![11, 12],
            sequence: 42,
            whispering: true,
        },
        Packet::Authenticate {
            player: 3,
            key: [4; 16],
        },
        Packet::AuthenticateAck,
        Packet::Ping {
            id: 5,
            timestamp: 6,
        },
        Packet::KeepAlive,
        Packet::ConnectionCheck,
        Packet::ConnectionCheckAck,
    ];
    let fixtures = vec![
        hex("01020b0c000000000000002a01"),
        [vec![5], 3u128.to_be_bytes().to_vec(), vec![4; 16]].concat(),
        vec![6],
        [
            vec![7],
            5u128.to_be_bytes().to_vec(),
            6i64.to_be_bytes().to_vec(),
        ]
        .concat(),
        vec![8],
        vec![9],
        vec![10],
    ];
    for (p, bytes) in packets.into_iter().zip(fixtures) {
        assert_eq!(p.encode(), bytes);
        assert_eq!(Packet::decode(&bytes).unwrap(), p);
    }
    for (id, placement) in [
        (2, Placement::Player),
        (3, Placement::Group),
        (4, Placement::Location([1.0, 2.0, 3.0])),
    ] {
        let mut s = sound(7);
        s.placement = placement;
        if id == 3 {
            s.distance = 0.0;
        }
        let mut bytes = vec![id];
        bytes.extend(1u128.to_be_bytes());
        bytes.extend(2u128.to_be_bytes());
        if id == 4 {
            for v in [1f64, 2., 3.] {
                bytes.extend(v.to_be_bytes());
            }
        }
        bytes.extend([2, 3, 4]);
        bytes.extend(7i64.to_be_bytes());
        if id != 3 {
            bytes.extend(48f32.to_be_bytes());
        }
        bytes.extend([3, 6]);
        bytes.extend(b"speech");
        let p = Packet::Sound(s);
        assert_eq!(p.encode(), bytes);
        assert_eq!(Packet::decode(&bytes).unwrap(), p);
        for end in 0..bytes.len() {
            assert!(Packet::decode(&bytes[..end]).is_err());
        }
    }
}
#[test]
fn secret_payload() {
    let mut bytes = vec![9; 16];
    bytes.extend(24454i32.to_be_bytes());
    bytes.extend(2u128.to_be_bytes());
    bytes.push(0);
    bytes.extend(1275i32.to_be_bytes());
    bytes.extend(48f64.to_be_bytes());
    bytes.extend(1000i32.to_be_bytes());
    bytes.extend([1, 0, 1]);
    let s = Secret::decode(&bytes).unwrap();
    assert_eq!(
        (s.port, s.player, s.keep_alive, s.host.as_str()),
        (24454, 2, 1000, "")
    );
    for end in 0..bytes.len() {
        assert!(Secret::decode(&bytes[..end]).is_err());
    }
}
#[test]
fn voice_host_authorities() {
    for (input, host, port) in [
        ("", "127.0.0.1", 24454),
        ("1234", "127.0.0.1", 1234),
        ("voice.example", "voice.example", 24454),
        ("voice.example:42", "voice.example", 42),
        ("[::1]:42", "::1", 42),
        ("[::1]", "::1", 24454),
    ] {
        assert_eq!(
            voice_host(input, "127.0.0.1", 24454).unwrap(),
            (host.into(), port)
        );
    }
    for bad in ["0", "host:0", "[::1", "host:no", "user@host"] {
        assert!(voice_host(bad, "localhost", 24454).is_err());
    }
}
#[test]
fn reorder_ordering_loss_duplicates() {
    let mut b = Reorder::default();
    assert_eq!(b.push(sound(10))[0].as_ref().unwrap().sequence, 10);
    assert!(b.push(sound(12)).is_empty());
    let ready = b.push(sound(11));
    assert_eq!(
        ready
            .iter()
            .map(|s| s.as_ref().unwrap().sequence)
            .collect::<Vec<_>>(),
        [11, 12]
    );
    assert!(b.push(sound(12)).is_empty());
    b.push(sound(15));
    let lost = b.drain(true);
    assert_eq!(lost.len(), 3);
    assert!(lost[0].is_none() && lost[1].is_none());
    assert_eq!(lost[2].as_ref().unwrap().sequence, 15);
}
#[test]
fn opus_round_trip_and_plc() {
    use rusty_opus::{Application, OpusEncoder};
    let mut e = OpusEncoder::new(48000, 1, Application::Voip).unwrap();
    let pcm: Vec<_> = (0..960)
        .map(|i| ((i as f32) * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.3)
        .collect();
    let mut packet = [0; 1275];
    let n = e.encode(&pcm, 960, &mut packet).unwrap();
    let mut d = OpusDecoder::new(48000, 1).unwrap();
    let mut out = [0.; 960];
    assert_eq!(d.decode(&packet[..n], 960, &mut out).unwrap(), 960);
    assert!(out.iter().any(|v| v.abs() > 0.01));
    assert_eq!(d.decode(&[], 960, &mut out).unwrap(), 960);
    assert!(out.iter().all(|s| s.is_finite()));
}
