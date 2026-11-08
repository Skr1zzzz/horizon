//! Channel declarations implemented from a written interoperability description.
use horizon_protocol::{Error, Result, buf::Reader};

pub fn channels(channel: &str, data: &[u8]) -> Result<Vec<String>> {
    if matches!(channel, "minecraft:register" | "minecraft:unregister") {
        return data
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| {
                std::str::from_utf8(s)
                    .map(String::from)
                    .map_err(|_| Error::new("Invalid channel UTF-8"))
            })
            .collect();
    }
    let mut r = Reader::new(data);
    if channel != "c:register" || r.varint()? != 1 {
        return Err(Error::new("Unknown registration"));
    }
    let phase = r.string(32)?;
    let mut out = Vec::new();
    for _ in 0..r.count(8192)? {
        out.push(r.string(128)?);
    }
    if r.remaining() != 0 {
        return Err(Error::new("Trailing registration bytes"));
    }
    Ok(if phase == "play" { out } else { Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_registration_forms() {
        let expected = vec![
            "voicechat:request_secret".to_owned(),
            "voicemessages:version".to_owned(),
        ];
        assert_eq!(
            channels(
                "minecraft:register",
                b"voicechat:request_secret\0voicemessages:version"
            )
            .unwrap(),
            expected
        );
        assert_eq!(
            channels(
                "c:register",
                b"\x01\x04play\x02\x18voicechat:request_secret\x15voicemessages:version"
            )
            .unwrap(),
            expected
        );
        assert!(
            channels("c:register", b"\x01\x06config\x00")
                .unwrap()
                .is_empty()
        );
        assert!(channels("c:register", b"\x01\x04play\x01").is_err());
    }
}
