//! SkinShuffle's small pause overlay; uploads never block the render thread.
use super::Game;
use horizon_client::{hud::HudBatch, skinshuffle as wire, text::Text, ui};
use horizon_protocol::packets::{configuration::CustomPayload, play::Serverbound as S};
use winit::keyboard::KeyCode;
#[derive(Default)]
pub(super) struct State {
    gate: wire::Gate,
    pub open: bool,
    field: Option<ui::EditBox>,
    slim: bool,
    status: String,
    job: Option<std::sync::mpsc::Receiver<Result<wire::Property, String>>>,
    hook_used: bool,
}
impl Game<'_> {
    pub(super) fn skin_ready(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|r| r.skinshuffle.gate.allow())
    }
    pub(super) fn skin_open(&self) -> bool {
        self.remote.as_ref().is_some_and(|r| r.skinshuffle.open)
    }
    pub(super) fn register_skinshuffle(&mut self) {
        if let Some(r) = self.remote.as_mut() {
            r.skinshuffle.gate.registered = true;
        }
        wire::trace(
            "send minecraft:register skinshuffle:handshake, skinshuffle:refresh_player_list_entry",
        );
        self.send(S::CustomPayload(CustomPayload {
            channel: "minecraft:register".into(),
            data: wire::CHANNELS.to_vec(),
        }));
    }
    pub(super) fn receive_skinshuffle(&mut self, channel: &str, data: &[u8]) {
        match wire::decode(channel, data) {
            Ok(p) => {
                wire::trace(format!("receive {channel} {p:?}"));
                if let Some(r) = self.remote.as_mut() {
                    match p {
                        wire::Incoming::Handshake => r.skinshuffle.gate.ready = true,
                        wire::Incoming::Refresh(id) => {
                            r.level_player_textures.remove(&id);
                        }
                    }
                }
                self.update_remote_skins();
            }
            Err(e) => log::warn!("SkinShuffle malformed {channel}: {e}"),
        }
    }
    fn start_skin(&mut self, source: String, slim: bool) {
        let Some(s) = self.remote.as_mut().map(|r| &mut r.skinshuffle) else {
            return;
        };
        if !s.gate.allow() || s.job.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        s.job = Some(rx);
        s.status = horizon_client::lang::get("horizon.skin.working");
        std::thread::spawn(move || {
            let _ = tx.send(wire::apply(&source, slim));
        });
    }
    pub(super) fn tick_skinshuffle(&mut self) {
        if !self.skin_ready() {
            return;
        }
        let s = &mut self.remote.as_mut().unwrap().skinshuffle;
        if !s.hook_used {
            s.hook_used = true;
            if let Ok(source) = std::env::var("HORIZON_SKINSHUFFLE_APPLY") {
                let (source, slim) = source
                    .strip_suffix("|slim")
                    .map_or((source.as_str(), false), |s| (s, true));
                self.start_skin(source.to_owned(), slim);
            }
        }
        let s = &mut self.remote.as_mut().unwrap().skinshuffle;
        let result = s.job.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(v) => Some(v),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("skin worker stopped".into()))
            }
            _ => None,
        });
        if let Some(result) = result {
            s.job = None;
            match result {
                Ok(p) => {
                    s.status = horizon_client::lang::get("horizon.skin.done");
                    wire::trace(format!(
                        "send {} has_signature={} name=textures value_len={} signature_len={}",
                        wire::APPLY,
                        p.signature.is_some(),
                        p.value.len(),
                        p.signature.as_ref().map_or(0, String::len)
                    ));
                    self.send(S::CustomPayload(CustomPayload {
                        channel: wire::APPLY.into(),
                        data: p.encode(),
                    }));
                }
                Err(e) => {
                    log::warn!("SkinShuffle apply: {e}");
                    s.status = e;
                }
            }
        }
    }
    pub(super) fn open_skin(&mut self) {
        if !self.skin_ready() {
            return;
        }
        let s = &mut self.remote.as_mut().unwrap().skinshuffle;
        s.open = true;
        if s.field.is_none() {
            let mut f = ui::EditBox::new(0.0, 0.0, 280.0, 20.0);
            f.max_length = 4096;
            f.focused = true;
            s.field = Some(f);
        }
    }
    pub(super) fn draw_skin(&mut self, batch: &mut HudBatch) {
        let s = &mut self.remote.as_mut().unwrap().skinshuffle;
        let atlas = &self.gui.atlas;
        horizon_client::widgets::begin_screen(|| {
            horizon_client::widgets::ScreenInfo::new(
                "skinshuffle",
                Text::translate("horizon.skin.title", &[]).plain(),
            )
        });
        let x = (batch.width - 280.0) / 2.0;
        let y = batch.height / 2.0 - 40.0;
        ui::centered_text(
            batch,
            atlas,
            &Text::translate("horizon.skin.title", &[]),
            x,
            y - 26.0,
            280.0,
            u32::MAX,
        );
        let f = s.field.as_mut().unwrap();
        f.x = x;
        f.y = y;
        f.draw(batch, atlas, None);
        for (i, key) in [
            if s.slim {
                "horizon.skin.slim"
            } else {
                "horizon.skin.classic"
            },
            "horizon.skin.apply",
            "gui.back",
        ]
        .into_iter()
        .enumerate()
        {
            let bx = x + i as f32 * 94.0;
            ui::button(
                batch,
                atlas,
                bx,
                y + 28.0,
                90.0,
                20.0,
                &Text::translate(key, &[]),
                i != 1 || s.job.is_none(),
                ui::hovered(self.cursor, bx, y + 28.0, 90.0, 20.0),
            );
        }
        batch.text(atlas, &s.status, x, y + 56.0, u32::MAX, true);
    }
    pub(super) fn skin_click(&mut self) {
        let s = &mut self.remote.as_mut().unwrap().skinshuffle;
        let f = s.field.as_mut().unwrap();
        let (x, y) = (f.x, f.y);
        if ui::hovered(self.cursor, x, y, 280.0, 20.0) {
            f.click(&self.gui.atlas, self.cursor.0, false, false);
            return;
        }
        if !ui::hovered(self.cursor, x, y + 28.0, 280.0, 20.0) {
            return;
        }
        match ((self.cursor.0 - x) / 94.0) as usize {
            0 => s.slim = !s.slim,
            1 => {
                let source = f.value.clone();
                let slim = s.slim;
                self.start_skin(source, slim);
            }
            _ => s.open = false,
        }
    }
    pub(super) fn skin_shortcut(&mut self, shortcut: horizon_client::clipboard::EditShortcut) {
        if let Some(f) = self
            .remote
            .as_mut()
            .and_then(|r| r.skinshuffle.field.as_mut())
        {
            f.shortcut(shortcut, &mut self.clipboard);
        }
    }
    pub(super) fn skin_key(&mut self, key: KeyCode, text: Option<&str>) {
        let s = &mut self.remote.as_mut().unwrap().skinshuffle;
        if key == KeyCode::Escape {
            s.open = false;
            return;
        }
        let f = s.field.as_mut().unwrap();
        f.key(key, false, false);
        if let Some(t) = text {
            f.insert_text(t);
        }
    }
}
