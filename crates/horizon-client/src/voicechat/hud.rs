//! Presentation priority keeps connection failures visible even while muted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Disconnected,
    Disabled,
    Muted,
    Talking,
}
pub fn state(connected: bool, muted: bool, disabled: bool, talking: bool) -> Option<Icon> {
    if !connected {
        Some(Icon::Disconnected)
    } else if disabled {
        Some(Icon::Disabled)
    } else if muted {
        Some(Icon::Muted)
    } else if talking {
        Some(Icon::Talking)
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hud_state_priority() {
        for muted in [false, true] {
            for disabled in [false, true] {
                for talking in [false, true] {
                    assert_eq!(
                        state(false, muted, disabled, talking),
                        Some(Icon::Disconnected)
                    );
                    let expected = if disabled {
                        Some(Icon::Disabled)
                    } else if muted {
                        Some(Icon::Muted)
                    } else if talking {
                        Some(Icon::Talking)
                    } else {
                        None
                    };
                    assert_eq!(state(true, muted, disabled, talking), expected);
                }
            }
        }
    }
}
