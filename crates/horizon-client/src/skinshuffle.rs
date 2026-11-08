//! SkinShuffle wire compatibility, adapted from SkinShuffle (MIT).
//! Protocol described by SkinShuffle 2.10.2; see THIRD_PARTY_NOTICES.md.
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};
use serde_json::{Value, json};
pub const HANDSHAKE: &str = "skinshuffle:handshake";
pub const REFRESH: &str = "skinshuffle:refresh_player_list_entry";
pub const APPLY: &str = "skinshuffle:skin_refresh";
pub const CHANNELS: &[u8] = b"skinshuffle:handshake\0skinshuffle:refresh_player_list_entry";
pub fn trace(message: impl std::fmt::Display) {
    if std::env::var("HORIZON_SKINSHUFFLE_LOG").as_deref() == Ok("1") {
        log::info!("SkinShuffle: {message}");
    }
}
#[derive(Debug, PartialEq)]
pub enum Incoming {
    Handshake,
    Refresh(i32),
}
pub fn decode(channel: &str, bytes: &[u8]) -> Result<Incoming> {
    let mut r = Reader::new(bytes);
    let p = match channel {
        HANDSHAKE => Incoming::Handshake,
        REFRESH => Incoming::Refresh(r.varint()?),
        _ => return Err(Error::new("unknown SkinShuffle channel")),
    };
    if r.remaining() != 0 {
        return Err(Error::new("trailing SkinShuffle bytes"));
    }
    Ok(p)
}
#[derive(Debug, PartialEq)]
pub struct Property {
    pub value: String,
    pub signature: Option<String>,
}
impl Property {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bool(self.signature.is_some());
        w.string("textures");
        w.string(&self.value);
        if let Some(s) = &self.signature {
            w.string(s);
        }
        w.data
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let signed = r.bool()?;
        if r.string(32767)? != "textures" {
            return Err(Error::new("expected textures property"));
        }
        let p = Self {
            value: r.string(32767)?,
            signature: if signed { Some(r.string(32767)?) } else { None },
        };
        if r.remaining() != 0 {
            return Err(Error::new("trailing property bytes"));
        }
        Ok(p)
    }
}
pub fn request(url: Option<&str>, slim: bool) -> Value {
    json!({"type": if url.is_some() {"url"} else {"file"}, "url": url, "model": if slim {"slim"} else {"classic"}})
}
pub fn reply(text: &str) -> std::result::Result<Property, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if v["usesDefaultSkin"].as_bool() != Some(false) {
        return Err("proxy returned default skin".into());
    }
    let field = |k: &str| {
        v[k].as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("proxy missing {k}"))
    };
    Ok(Property {
        value: field("textureValue")?,
        signature: Some(field("textureSignature")?),
    })
}
pub fn apply(source: &str, slim: bool) -> std::result::Result<Property, String> {
    let url = source.starts_with("https://").then_some(source);
    if source.contains("://") && url.is_none() {
        return Err("skin URL must use https".into());
    }
    let file = if url.is_none() {
        let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("expected PNG (maximum 4 MiB)".into());
        }
        Some(bytes)
    } else {
        None
    };
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .build();
    if let Some(session) = crate::auth::current() {
        trace("apply online: Mojang upload then proxy + refresh");
        let post = agent
            .post("https://api.minecraftservices.com/minecraft/profile/skins")
            .set(
                "Authorization",
                &format!("Bearer {}", session.access_token()),
            );
        let variant = if slim { "slim" } else { "classic" };
        if let Some(url) = url {
            post.set("Content-Type", "application/json")
                .send_string(&json!({"variant": variant, "url": url}).to_string())
                .map_err(|e| format!("Mojang upload: {e}"))?;
        } else {
            let boundary = "HorizonSkinShuffleBoundary";
            let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"variant\"\r\n\r\n{variant}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
            body.extend(file.as_ref().unwrap());
            body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
            post.set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body)
            .map_err(|e| format!("Mojang upload: {e}"))?;
        }
    } else {
        trace("apply offline: skip Mojang, proxy + refresh");
    }
    let (mut socket, _) = tungstenite::connect("wss://skinshuffle.imb11.dev/skin-gateway")
        .map_err(|e| e.to_string())?;
    let result = (|| {
        if let Some(bytes) = file {
            socket
                .send(tungstenite::Message::Binary(bytes.into()))
                .map_err(|e| e.to_string())?;
        }
        socket
            .send(tungstenite::Message::Text(
                request(url, slim).to_string().into(),
            ))
            .map_err(|e| e.to_string())?;
        loop {
            match socket.read().map_err(|e| e.to_string())? {
                tungstenite::Message::Text(text) => return reply(&text),
                tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {}
                _ => return Err("proxy closed without a skin reply".into()),
            }
        }
    })();
    let _ = socket.close(None);
    result
}
/// Resolve the newest listed skin while retaining NPC skins during list removal.
pub fn skin_entries(
    players: &std::collections::HashMap<i32, u128>,
    listed: &std::collections::HashMap<u128, crate::skins::Textures>,
    kept: &mut std::collections::HashMap<i32, crate::skins::Textures>,
) -> Vec<(u128, crate::skins::Textures)> {
    for (id, uuid) in players {
        if let Some(textures) = listed.get(uuid) {
            kept.insert(*id, textures.clone());
        }
    }
    let mut entries: Vec<_> = listed.iter().map(|(u, t)| (*u, t.clone())).collect();
    for (id, textures) in kept.iter() {
        if let Some(uuid) = players.get(id)
            && !listed.contains_key(uuid)
            && !entries.iter().any(|(u, _)| u == uuid)
        {
            entries.push((*uuid, textures.clone()));
        }
    }
    entries
}
#[derive(Default)]
pub struct Gate {
    pub registered: bool,
    pub ready: bool,
}
impl Gate {
    pub fn allow(&self) -> bool {
        self.registered && self.ready
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skin_payload_vectors() {
        assert_eq!(decode(HANDSHAKE, &[]).unwrap(), Incoming::Handshake);
        assert!(decode(HANDSHAKE, &[0]).is_err());
        assert_eq!(decode(REFRESH, &[0xac, 2]).unwrap(), Incoming::Refresh(300));
        assert!(decode(REFRESH, &[0x80]).is_err());
        let p = Property {
            value: "v".into(),
            signature: Some("s".into()),
        };
        assert_eq!(p.encode(), b"\x01\x08textures\x01v\x01s");
        assert_eq!(Property::decode(&p.encode()).unwrap(), p);
        let p = Property {
            value: "v".into(),
            signature: None,
        };
        assert_eq!(p.encode(), b"\x00\x08textures\x01v");
        assert_eq!(Property::decode(&p.encode()).unwrap(), p);
    }
    #[test]
    fn skin_proxy_contract() {
        assert_eq!(
            request(None, false),
            json!({"type":"file","url":null,"model":"classic"})
        );
        assert_eq!(
            request(Some("https://example.org/s.png"), true),
            json!({"type":"url","url":"https://example.org/s.png","model":"slim"})
        );
        assert_eq!(
            reply(r#"{"usesDefaultSkin":false,"textureValue":"v","textureSignature":"s"}"#)
                .unwrap()
                .signature
                .as_deref(),
            Some("s")
        );
        assert!(reply(r#"{"usesDefaultSkin":true}"#).is_err());
        assert!(reply(r#"{"usesDefaultSkin":false,"textureValue":"v"}"#).is_err());
    }
    #[test]
    fn skin_refresh_on_playerinfo_readd() {
        use std::collections::HashMap;
        let old = crate::skins::Textures {
            skin: Some("https://textures.minecraft.net/texture/old".into()),
            ..Default::default()
        };
        let new = crate::skins::Textures {
            skin: Some("https://textures.minecraft.net/texture/new".into()),
            slim: true,
            ..Default::default()
        };
        let players = HashMap::from([(42, 7)]);
        let mut listed = HashMap::from([(7, old.clone())]);
        let mut kept = HashMap::new();
        assert_eq!(skin_entries(&players, &listed, &mut kept), [(7, old)]);
        listed.insert(7, new.clone());
        assert_eq!(
            skin_entries(&players, &listed, &mut kept),
            [(7, new.clone())]
        );
        listed.clear();
        assert_eq!(skin_entries(&players, &listed, &mut kept), [(7, new)]);
    }
    #[test]
    fn skin_handshake_gate() {
        let mut g = Gate::default();
        assert!(!g.allow());
        g.registered = true;
        assert!(!g.allow());
        g.ready = true;
        assert!(g.allow());
    }
}
