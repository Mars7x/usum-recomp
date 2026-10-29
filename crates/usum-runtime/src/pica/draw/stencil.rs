//! PICA's masked stencil comparison and the three fragment-result operations.

#[derive(Clone, Copy)]
pub(super) struct Stencil {
    pub enabled: bool,
    reference: u8,
    read_mask: u8,
    write_mask: u8,
    function: u32,
    operations: [u8; 3],
}

impl Stencil {
    pub fn new(test: u32, operations: u32, permissions: u32, format: u32) -> Self {
        Self {
            enabled: format == 3 && test & 1 != 0,
            reference: (test >> 16) as u8,
            read_mask: (test >> 24) as u8,
            write_mask: if permissions & 1 != 0 { (test >> 8) as u8 } else { 0 },
            function: (test >> 4) & 7,
            operations: [operations as u8 & 7, (operations >> 4) as u8 & 7, (operations >> 8) as u8 & 7],
        }
    }

    pub fn passes(self, stored: u8) -> bool {
        !self.enabled
            || super::compare(self.function, u32::from(self.reference & self.read_mask), u32::from(stored & self.read_mask))
    }

    pub fn changes(self, outcome: u8) -> bool {
        self.enabled && self.write_mask != 0 && self.operations[usize::from(outcome)] != 0
    }

    // Outcomes: stencil failure, depth failure, both tests passed.
    pub fn apply(self, outcome: u8, stored: u8) -> u8 {
        let value = match self.operations[usize::from(outcome)] {
            0 => stored,
            1 => 0,
            2 => self.reference,
            3 => stored.saturating_add(1),
            4 => stored.saturating_sub(1),
            5 => !stored,
            6 => stored.wrapping_add(1),
            _ => stored.wrapping_sub(1),
        };
        (stored & !self.write_mask) | (value & self.write_mask)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn render(test: u32, ops: u32, stored: u8, depth_test: u32, alpha_test: u32, permissions: u32) -> (Vec<u8>, Vec<u8>) {
        let mut p = Pica::default();
        p.regs[0x41] = 0x410000;
        p.regs[0x43] = 0x410000;
        p.regs[0x4d] = 0xbf0000;
        p.regs[0x6d] = 1;
        p.regs[0x104] = alpha_test;
        p.regs[0x105] = test;
        p.regs[0x106] = ops;
        p.regs[0x107] = 0xf00 | depth_test;
        p.regs[0x113] = 0xf;
        p.regs[0x115] = permissions;
        let vertex = |x, y| Vertex { position: [x, y, -0.5, 1.0], color: [1.0; 4], ..Vertex::default() };
        let v = [vertex(-1.0, -1.0), vertex(1.0, -1.0), vertex(1.0, 1.0), vertex(-1.0, 1.0)];
        let triangles = [prepare(&p, [&v[0], &v[1], &v[2]]).unwrap(), prepare(&p, [&v[0], &v[2], &v[3]]).unwrap()];
        let target = Target { color: 0, depth: 0, width: 8, height: 8, color_format: 0, depth_format: 3 };
        let mut color = vec![0; 256];
        let mut depth = [255, 255, 255, stored].repeat(64);
        raster_band(
            &p,
            (&[None, None, None], None),
            &target,
            &triangles,
            Band { first_row: 0, rows: 8, first_col: 0, cols: 8, color: &mut color, depth: &mut depth },
        );
        assert!(depth.as_chunks::<4>().0.iter().all(|p| p[..3] == [255; 3]), "stencil writes must preserve depth");
        (color, depth)
    }

    #[test]
    fn stencil_operations_masks_and_permissions_preserve_other_bits() {
        for (operation, stored, expected) in
            [(0, 128, 128), (1, 128, 0), (2, 128, 0x91), (3, 255, 255), (4, 0, 0), (5, 0x5a, 0xa5), (6, 255, 0), (7, 0, 255)]
        {
            let (color, depth) = render(0xff91ff11, operation << 8, stored, 0, 0, 1);
            assert!(color.iter().all(|&b| b == 255));
            assert!(depth.as_chunks::<4>().0.iter().all(|p| p[3] == expected), "operation {operation}");
        }
        for (test, permissions, expected) in [(0xff910f11, 1, 0xa1), (0xff91ff11, 0, 0xa5), (0xff91ff10, 1, 0xa5)] {
            let (_, depth) = render(test, 2 << 8, 0xa5, 0, 0, permissions);
            assert!(depth.as_chunks::<4>().0.iter().all(|p| p[3] == expected));
        }
        assert!(!super::Stencil::new(0xff91ff11, 2 << 8, 1, 0).enabled);
    }

    #[test]
    fn stencil_failure_depth_failure_and_alpha_rejection_select_the_right_writes() {
        for (function, stored, reference, passes) in [
            (0, 9, 9, false),
            (1, 9, 9, true),
            (2, 9, 9, true),
            (2, 8, 9, false),
            (3, 8, 9, true),
            (3, 9, 9, false),
            (4, 10, 9, true),
            (4, 8, 9, false),
            (5, 9, 9, true),
            (5, 8, 9, false),
            (6, 8, 9, true),
            (6, 10, 9, false),
            (7, 9, 9, true),
            (7, 10, 9, false),
        ] {
            let (color, depth) = render(0xff00ff01 | (reference << 16) | (function << 4), 2 << 8, stored, 0, 0, 1);
            assert!(color.iter().all(|&b| b == if passes { 255 } else { 0 }), "comparison {function}");
            assert!(depth.as_chunks::<4>().0.iter().all(|p| p[3] == if passes { reference as u8 } else { stored }));
        }
        let (color, depth) = render(0x0f910f21, 2 << 8, 0xa1, 0, 0, 1);
        assert!(color.iter().all(|&b| b == 255), "compare masked reference against masked buffer");
        assert!(depth.as_chunks::<4>().0.iter().all(|p| p[3] == 0xa1));
        for (test, ops, depth_test) in [(0xff91ff01, 2, 0), (0xff91ff11, 2 << 4, 1), (0xff91ff11, 2 << 8, 0)] {
            for alpha_reject in [false, true] {
                let (color, depth) = render(test, ops, 0x5a, depth_test, u32::from(alpha_reject), 1);
                assert!(depth.as_chunks::<4>().0.iter().all(|p| p[3] == if alpha_reject { 0x5a } else { 0x91 }));
                let passes = test & 0xf0 != 0 && depth_test == 0 && !alpha_reject;
                assert!(color.iter().all(|&b| b == if passes { 255 } else { 0 }));
            }
        }
    }
}
