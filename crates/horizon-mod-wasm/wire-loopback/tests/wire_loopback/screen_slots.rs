//! `ScreensClientHost::set_screen_slot` and `screen_slot` from a wasm mod reach the
//! client with their arguments, and each named refusal comes back as the native call
//! returns it (#18397).
use super::*;
use mod_api::ScreenSlotRefusal;
use mod_api::prelude::*;
use std::cell::{Cell, RefCell};

/// A client whose open container screen has two slots, the second hidden when it opened.
struct Client {
    open: Cell<bool>,
    places: RefCell<Vec<Option<[f32; 2]>>>,
}
impl mod_api::Host for Client {}
mod_api::impl_host_areas!(Client);
impl mod_api::ClientHost for Client {}
mod_api::impl_client_host_areas!(Client, except(screens));
impl ScreensClientHost for Client {
    fn screen_slot(&self, slot: usize) -> Option<[f32; 2]> {
        self.places.borrow().get(slot).copied().flatten()
    }
    fn set_screen_slot(&self, slot: usize, pos: Option<[f32; 2]>) -> Result<(), ScreenSlotRefusal> {
        if !self.open.get() {
            return Err(ScreenSlotRefusal::NoScreen);
        }
        let mut places = self.places.borrow_mut();
        let place = places.get_mut(slot).ok_or(ScreenSlotRefusal::NoSuchSlot)?;
        if slot == 1 && pos.is_some() {
            return Err(ScreenSlotRefusal::HiddenAtOpen);
        }
        if pos.is_some_and(|[x, y]| !(x.abs() <= 1.0e6 && y.abs() <= 1.0e6)) {
            return Err(ScreenSlotRefusal::BadPosition);
        }
        *place = pos;
        Ok(())
    }
}

#[test]
fn a_wasm_mods_slot_move_crosses_the_wire_and_each_refusal_comes_back_by_name() {
    mod_api::__private::initialize().unwrap();
    let _phase = mod_api::__private::PhaseOwner::scope(mod_api::__private::ModPhase::World);
    let mut client = Client {
        open: Cell::new(true),
        places: RefCell::new(vec![Some([8.0, 142.0]), None]),
    };
    let mut frame = boundary::Frame::default();
    frame.set_client_host_mut(&mut client);
    let mut cx = boundary::Cx::default();
    cx.frames.push(frame);
    let cx = RefCell::new(cx);
    guest::set_transport(Some(Rc::new(move |id, bytes| {
        let mut out = Writer::new();
        // SAFETY: client and its frame remain live for this synchronous transport.
        unsafe { boundary::serve(id, &mut *cx.borrow_mut(), bytes, &mut out) }.unwrap();
        out.into_bytes()
    })));
    assert_eq!(guest::Guest.screen_slot(0), Some([8.0, 142.0]));
    assert_eq!(
        guest::Guest.set_screen_slot(0, Some([180.5, -20.0])),
        Ok(())
    );
    assert_eq!(guest::Guest.screen_slot(0), Some([180.5, -20.0]));
    assert_eq!(
        guest::Guest.set_screen_slot(1, Some([0.0, 0.0])),
        Err(ScreenSlotRefusal::HiddenAtOpen)
    );
    assert_eq!(
        guest::Guest.set_screen_slot(2, None),
        Err(ScreenSlotRefusal::NoSuchSlot)
    );
    assert_eq!(
        guest::Guest.set_screen_slot(0, Some([f32::NAN, 0.0])),
        Err(ScreenSlotRefusal::BadPosition)
    );
    assert_eq!(guest::Guest.set_screen_slot(0, None), Ok(()));
    assert_eq!(guest::Guest.screen_slot(0), None);
    client.open.set(false);
    assert_eq!(
        guest::Guest.set_screen_slot(0, Some([8.0, 142.0])),
        Err(ScreenSlotRefusal::NoScreen)
    );
    guest::set_transport(None);
}
