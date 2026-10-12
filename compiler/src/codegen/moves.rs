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
