//! Per-draw assembly plan for hardware vertex inputs. Memory reads stay in the
//! original loader/field order, including attributes whose values are discarded.
use super::{element, element_in, Layout, Memory, Pica, Vec4};

pub(super) struct InputPlan<'a> {
    layout: &'a Layout,
    initial: [Vec4; 16],
    destinations: Vec<u8>,
}

impl<'a> InputPlan<'a> {
    pub(super) fn new(pica: &Pica, layout: &'a Layout) -> Self {
        let mut last = [usize::MAX; 12];
        let mut fields = 0;
        for loader in &layout.loaders {
            for &(attribute, _, _, _) in &loader.fields {
                last[attribute] = fields;
                fields += 1;
            }
        }
        let mut initial = [[0.0; 4]; 16];
        let mut owners = [None; 16];
        for attribute in 0..layout.total.min(12) {
            let register = usize::from(pica.vs.permutation[attribute] & 15);
            initial[register] = pica.fixed[attribute];
            owners[register] = Some(attribute);
        }
        let mut destinations = vec![u8::MAX; fields];
        for (register, attribute) in owners.into_iter().enumerate() {
            if let Some(attribute) = attribute {
                if last[attribute] != usize::MAX {
                    destinations[last[attribute]] = register as u8;
                }
            }
        }
        Self { layout, initial, destinations }
    }

    pub(super) fn load(&self, memory: &Memory, index: u32) -> Result<[Vec4; 16], String> {
        let mut inputs = self.initial;
        let mut first = 0;
        for loader in &self.layout.loaders {
            let start = loader.start.wrapping_add(index.wrapping_mul(loader.stride));
            let bytes = memory.slice(start, loader.span);
            let destinations = &self.destinations[first..first + loader.fields.len()];
            first += loader.fields.len();
            for (&(_, kind, elements, at), &destination) in loader.fields.iter().zip(destinations) {
                let size = [1, 1, 2, 4][kind as usize];
                let mut value = [0.0, 0.0, 0.0, 1.0];
                for (element_index, slot) in value.iter_mut().enumerate().take(elements as usize) {
                    let offset = at + element_index as u32 * size;
                    *slot = match bytes {
                        Some(bytes) => element_in(bytes, offset as usize, kind),
                        None => element(memory, start.wrapping_add(offset), kind)?,
                    };
                }
                if destination != u8::MAX {
                    inputs[usize::from(destination)] = value;
                }
            }
        }
        Ok(inputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pica::draw::{load_vertex, Loader};
    use usum_guest::Perm;

    fn bits(result: Result<[Vec4; 16], String>) -> Result<[[u32; 4]; 16], String> {
        result.map(|registers| registers.map(|v| v.map(f32::to_bits)))
    }

    fn next(seed: &mut u64) -> u32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*seed >> 32) as u32
    }

    #[test]
    fn planned_inputs_match_reference_bits_formats_aliases_and_wrapping_reads() {
        let mut seed = 0x9134756;
        let mut memory = Memory::new();
        for (address, size, perm) in [
            (0, 256, Perm::RW),
            (0x1000, 4096, Perm::RW),
            (0x2000, 4096, Perm::RW),
            (0x4000, 4096, Perm::R),
            (0x6000, 4096, Perm { read: false, write: true }),
            (0xffff_ff00, 256, Perm::RW),
        ] {
            memory.map(address, (0..size).map(|_| next(&mut seed) as u8).collect(), perm).unwrap();
        }
        for address in [0x8000, 0x9000, 0xa000, 0xb000] {
            memory.map(address, vec![0; 4096], Perm::R).unwrap();
        }
        for _ in 0..256 {
            let mut pica = Pica::default();
            for value in pica.fixed.iter_mut().flatten() {
                *value = f32::from_bits(next(&mut seed));
            }
            for register in &mut pica.vs.permutation {
                *register = (next(&mut seed) % 5) as u8 | 0xa0;
            }
            let mut loaders = Vec::new();
            for _ in 0..next(&mut seed) % 5 {
                let start =
                    [0, 0x1000, 0x1ff0, 0x2004, 0x2ff8, 0x4000, 0x6000, 0xffff_fff0, 0xdead_0000][next(&mut seed) as usize % 9];
                let stride = [0, 1, 7, 16, 64][next(&mut seed) as usize % 5];
                let mut span = 0u32;
                let mut fields = Vec::new();
                for _ in 0..next(&mut seed) % 13 {
                    let attribute = next(&mut seed) as usize % 12;
                    let kind = next(&mut seed) % 4;
                    let elements = next(&mut seed) % 4 + 1;
                    let size = [1, 1, 2, 4][kind as usize];
                    span = span.next_multiple_of(size);
                    fields.push((attribute, kind, elements, span));
                    span += elements * size;
                }
                loaders.push(Loader { start, stride, span, fields });
            }
            let layout = Layout { loaders, total: next(&mut seed) as usize % 17 };
            let plan = InputPlan::new(&pica, &layout);
            for index in [0, 1, 2, 15, 31, u32::MAX] {
                assert_eq!(bits(plan.load(&memory, index)), bits(load_vertex(&pica, &layout, &memory, index)));
            }
        }
    }

    #[test]
    fn last_loader_and_later_fixed_attribute_keep_their_alias_precedence() {
        let mut pica = Pica::default();
        pica.vs.permutation[0] = 0xf7;
        pica.vs.permutation[1] = 0x17;
        pica.fixed[1] = [f32::from_bits(0x7fc1_2345), -0.0, -2.0, 1.0];
        let first = [1.0f32, 2.0, 3.0, 4.0];
        let last = [-0.0f32, f32::from_bits(0x7fa1_2345), 5.0, 6.0];
        let mut memory = Memory::new();
        memory.map(0x1000, first.into_iter().chain(last).flat_map(f32::to_le_bytes).collect(), Perm::RW).unwrap();
        for total in [1, 2] {
            let layout = Layout {
                total,
                loaders: [0x1000, 0x1010]
                    .into_iter()
                    .map(|start| Loader { start, stride: 0, span: 16, fields: vec![(0, 3, 4, 0)] })
                    .collect(),
            };
            let output = bits(InputPlan::new(&pica, &layout).load(&memory, 0)).unwrap();
            assert_eq!(output, bits(load_vertex(&pica, &layout, &memory, 0)).unwrap());
            assert_eq!(output[7], if total == 1 { last } else { pica.fixed[1] }.map(f32::to_bits));
        }
    }

    #[test]
    fn overwritten_and_unused_attributes_still_report_the_first_memory_fault() {
        let pica = Pica::default();
        let mut memory = Memory::new();
        memory.map(0x1000, vec![0; 32], Perm::RW).unwrap();
        for total in [0, 1] {
            let layout = Layout {
                total,
                loaders: vec![
                    Loader { start: 0xdead_0000, stride: 16, span: 16, fields: vec![(0, 3, 4, 0)] },
                    Loader { start: 0x1000, stride: 16, span: 16, fields: vec![(0, 3, 4, 0)] },
                ],
            };
            let original = load_vertex(&pica, &layout, &memory, 0).unwrap_err();
            assert!(original.contains("0xdead0000"));
            assert_eq!(InputPlan::new(&pica, &layout).load(&memory, 0).unwrap_err(), original);
        }
    }

    #[test]
    fn deferred_loader_reads_keep_provider_order_and_faults_for_discarded_values() {
        use std::{
            any::Any,
            sync::{Arc, Mutex},
        };
        use usum_guest::{memory::DeferredBytes, Fault};
        #[derive(Debug)]
        struct Provider {
            label: u8,
            fail: bool,
            bytes: Vec<u8>,
            calls: Arc<Mutex<Vec<u8>>>,
        }
        impl DeferredBytes for Provider {
            fn len(&self) -> usize {
                self.bytes.len()
            }
            fn bytes(&self) -> Result<&[u8], Fault> {
                self.calls.lock().unwrap().push(self.label);
                if self.fail {
                    Err(Fault::Device { address: 0x1000 + u32::from(self.label) * 16 })
                } else {
                    Ok(&self.bytes)
                }
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
        for failure in [None, Some(0), Some(1)] {
            let pica = Pica::default();
            let layout = Layout {
                total: 1,
                loaders: vec![Loader { start: 0x1000, stride: 32, span: 32, fields: vec![(0, 3, 4, 0), (0, 3, 4, 16)] }],
            };
            let run = |planned| {
                let mut memory = Memory::new();
                memory.map(0x1000, vec![0; 32], Perm::RW).unwrap();
                let calls = Arc::new(Mutex::new(Vec::new()));
                for label in [0, 1] {
                    memory
                        .defer_device_write(
                            0x1000 + u32::from(label) * 16,
                            Arc::new(Provider {
                                label,
                                fail: failure == Some(label),
                                bytes: vec![label + 1; 16],
                                calls: Arc::clone(&calls),
                            }),
                        )
                        .unwrap();
                }
                let result = if planned {
                    InputPlan::new(&pica, &layout).load(&memory, 0)
                } else {
                    load_vertex(&pica, &layout, &memory, 0)
                };
                let trace = calls.lock().unwrap().clone();
                (bits(result), trace)
            };
            assert_eq!(run(false), run(true));
        }
    }
}
