/// A copy gesture consists of two separate C presses with Ctrl, within 650 ms.
/// Repeated keydown messages from a held key are not a second copy.
#[derive(Default)]
pub struct CopyGesture {
    controls: u8,
    shift: bool,
    alt: bool,
    c_down: bool,
    copy_on_release: bool,
    last_copy: Option<u64>,
}
impl CopyGesture {
    pub fn starts_copy(&self, key: u32, down: bool, now: u64) -> bool {
        key == 0x43
            && down
            && !self.c_down
            && self.controls != 0
            && !self.shift
            && !self.alt
            && self.last_copy.is_none_or(|t| now.saturating_sub(t) > 650)
    }
    pub fn key(&mut self, key: u32, down: bool, now: u64) -> bool {
        match key {
            0x11 | 0xa2 => {
                if down {
                    self.controls |= 1;
                } else {
                    self.controls &= !1;
                }
            }
            0xa3 => {
                if down {
                    self.controls |= 2;
                } else {
                    self.controls &= !2;
                }
            }
            0x10 | 0xa0 | 0xa1 => {
                self.shift = down;
                if down {
                    self.last_copy = None;
                }
            }
            0x12 | 0xa4 | 0xa5 => {
                self.alt = down;
                if down {
                    self.last_copy = None;
                }
            }
            0x43 => {
                if down {
                    if !self.c_down {
                        self.copy_on_release = self.controls != 0 && !self.shift && !self.alt;
                    }
                    self.c_down = true;
                } else if self.c_down {
                    self.c_down = false;
                    if self.copy_on_release {
                        self.copy_on_release = false;
                        let doubled = self.last_copy.is_some_and(|t| now.saturating_sub(t) <= 650);
                        self.last_copy = if doubled { None } else { Some(now) };
                        return doubled;
                    }
                }
            }
            _ => {
                if down {
                    self.last_copy = None;
                    self.copy_on_release = false;
                }
            }
        }
        false
    }
}

pub fn popup_position(
    cursor: (i32, i32),
    work: (i32, i32, i32, i32),
    size: (i32, i32),
) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    let (width, height) = size;
    let x = if cursor.0 + 12 + width <= right {
        cursor.0 + 12
    } else {
        cursor.0 - width - 12
    };
    let y = if cursor.1 + 18 + height <= bottom {
        cursor.1 + 18
    } else {
        cursor.1 - height - 18
    };
    (
        x.clamp(left + 4, (right - width - 4).max(left + 4)),
        y.clamp(top + 4, (bottom - height - 4).max(top + 4)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn copy(g: &mut CopyGesture, t: u64) -> bool {
        g.key(0x43, true, t);
        g.key(0x43, false, t + 10)
    }
    #[test]
    fn ctrl_c_c_triggers_once() {
        let mut g = CopyGesture::default();
        g.key(0xa2, true, 0);
        assert!(!copy(&mut g, 10));
        assert!(copy(&mut g, 200));
        assert!(!copy(&mut g, 300));
    }
    #[test]
    fn held_c_does_not_trigger() {
        let mut g = CopyGesture::default();
        g.key(0xa2, true, 0);
        for t in [10, 100, 200, 300] {
            assert!(!g.key(0x43, true, t));
        }
        assert!(!g.key(0x43, false, 400));
    }
    #[test]
    fn timeout_and_other_chords_do_not_trigger() {
        let mut g = CopyGesture::default();
        g.key(0xa2, true, 0);
        assert!(!copy(&mut g, 10));
        assert!(!copy(&mut g, 800));
        g.key(0x56, true, 850);
        assert!(!copy(&mut g, 900));
        g.key(0x10, true, 950);
        assert!(!copy(&mut g, 1000));
    }
    #[test]
    fn ctrl_can_be_released_between_copies() {
        let mut g = CopyGesture::default();
        g.key(0xa2, true, 0);
        assert!(!copy(&mut g, 10));
        g.key(0xa2, false, 30);
        g.key(0xa3, true, 100);
        assert!(copy(&mut g, 200));
    }
    #[test]
    fn popup_stays_on_negative_coordinate_monitor() {
        assert_eq!(
            popup_position((-10, 1050), (-1920, 0, 0, 1080), (440, 380)),
            (-462, 652)
        );
        let p = popup_position((5, 5), (0, 0, 800, 600), (440, 380));
        assert_eq!(p, (17, 23));
    }
}
