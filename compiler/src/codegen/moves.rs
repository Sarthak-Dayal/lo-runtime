//! Parallel register moves.
//!
//! Moving incoming parameters from their argument registers to their allocated
//! registers is a *parallel* assignment: `rsi <- rdi` and `rdi <- rsi` must both
//! read the old values. Done naively in sequence the second move reads a clobbered
//! register. `parallel_move` orders the moves and breaks cycles through `r11`
//! (a scratch register the allocator never uses).

use super::x86::{Inst, Operand, Reg, Width};

/// `dst <- src` at `width`. Every destination must be distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub dst: Reg,
    pub src: Reg,
    pub width: Width,
}

const SCRATCH: Reg = Reg::R11;

pub fn parallel_move(moves: &[Move]) -> Result<Vec<Inst>, String> {
    let mut pending: Vec<Move> = moves.iter().copied().filter(|m| m.dst != m.src).collect();
    for (i, m) in pending.iter().enumerate() {
        if m.dst == SCRATCH || m.src == SCRATCH {
            return Err("parallel move cannot use the scratch register r11".into());
        }
        if pending[..i].iter().any(|other| other.dst == m.dst) {
            return Err(format!("parallel move writes {:?} twice", m.dst));
        }
    }

    let mut out = vec![];
    while !pending.is_empty() {
        // A move is safe once no other pending move still needs to read its destination.
        let ready = pending
            .iter()
            .position(|m| !pending.iter().any(|other| other.src == m.dst));
        match ready {
            Some(index) => {
                let m = pending.remove(index);
                out.push(mov(m.dst, m.src, m.width));
            }
            None => {
                // Every destination is still a pending source: a cycle. Park one
                // destination's current value in the scratch register and let its
                // readers take it from there, which frees that destination.
                let victim = pending[0].dst;
                let width = pending
                    .iter()
                    .find(|m| m.src == victim)
                    .expect("a cycle has a reader")
                    .width;
                out.push(mov(SCRATCH, victim, width));
                for m in pending.iter_mut().filter(|m| m.src == victim) {
                    m.src = SCRATCH;
                }
            }
        }
    }
    Ok(out)
}

fn mov(dst: Reg, src: Reg, width: Width) -> Inst {
    Inst::Mov(Operand::Reg(dst, width), Operand::Reg(src, width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::print::inst_text;
    use std::collections::HashMap;
    use Reg::*;

    fn m(dst: Reg, src: Reg) -> Move {
        Move {
            dst,
            src,
            width: Width::W64,
        }
    }

    fn text(moves: &[Move]) -> Vec<String> {
        parallel_move(moves)
            .unwrap()
            .iter()
            .map(inst_text)
            .collect()
    }

    #[test]
    fn independent_moves_stay_in_order_and_identity_moves_vanish() {
        assert_eq!(
            text(&[m(Rbx, Rdi), m(R12, Rsi), m(R8, R8)]),
            ["mov rbx, rdi", "mov r12, rsi"]
        );
    }

    #[test]
    fn chains_are_ordered_so_no_source_is_clobbered() {
        // rdi <- rsi needs rsi's old value, so rsi <- r8 must wait for it.
        assert_eq!(
            text(&[m(Rsi, R8), m(Rdi, Rsi)]),
            ["mov rdi, rsi", "mov rsi, r8"]
        );
    }

    #[test]
    fn a_swap_goes_through_r11() {
        assert_eq!(
            text(&[m(Rsi, Rdi), m(Rdi, Rsi)]),
            ["mov r11, rsi", "mov rsi, rdi", "mov rdi, r11"]
        );
    }

    #[test]
    fn widths_are_kept_per_move() {
        let moves = [
            Move {
                dst: Rsi,
                src: Rdi,
                width: Width::W32,
            },
            Move {
                dst: Rdi,
                src: Rsi,
                width: Width::W32,
            },
        ];
        assert_eq!(
            text(&moves),
            ["mov r11d, esi", "mov esi, edi", "mov edi, r11d"]
        );
    }

    #[test]
    fn scratch_and_duplicate_destinations_are_rejected() {
        assert!(parallel_move(&[m(R11, Rdi)]).is_err());
        assert!(parallel_move(&[m(Rbx, Rdi), m(Rbx, Rsi)]).is_err());
    }

    /// Execute the emitted register moves and compare with the parallel meaning.
    fn check(moves: &[Move]) {
        let mut regs: HashMap<Reg, u64> = HashMap::new();
        let pool = [Rdi, Rsi, Rdx, Rcx, R8, R9, Rbx, R12, R13, R14, R15, R11];
        for (i, r) in pool.iter().enumerate() {
            regs.insert(*r, 100 + i as u64);
        }
        let initial = regs.clone();
        for inst in parallel_move(moves).unwrap() {
            let Inst::Mov(Operand::Reg(d, _), Operand::Reg(s, _)) = inst else {
                panic!("unexpected {inst:?}");
            };
            let value = regs[&s];
            regs.insert(d, value);
        }
        for mv in moves {
            assert_eq!(regs[&mv.dst], initial[&mv.src], "{moves:?}");
        }
    }

    #[test]
    fn every_injective_assignment_over_the_argument_registers_is_correct() {
        let sources = [Rdi, Rsi, R8, R9];
        let targets = [Rdi, Rsi, R8, R9, Rbx, R12];
        // For each source choose either "unused" or a distinct target.
        fn assign(
            index: usize,
            sources: &[Reg],
            targets: &[Reg],
            used: &mut Vec<Reg>,
            moves: &mut Vec<Move>,
            count: &mut usize,
        ) {
            if index == sources.len() {
                super::tests::check(moves);
                *count += 1;
                return;
            }
            // Parameter `index` is unused (its register is simply not moved).
            assign(index + 1, sources, targets, used, moves, count);
            for &target in targets {
                if used.contains(&target) {
                    continue;
                }
                used.push(target);
                moves.push(Move {
                    dst: target,
                    src: sources[index],
                    width: Width::W64,
                });
                assign(index + 1, sources, targets, used, moves, count);
                moves.pop();
                used.pop();
            }
        }
        let mut count = 0;
        assign(0, &sources, &targets, &mut vec![], &mut vec![], &mut count);
        assert!(count > 1000, "enumerated only {count} cases");
    }
}
