//! Behavioral test machine for semantic IR; deliberately does not simulate GC.
//! It executes target-width data, memory, dispatch, roots, and runtime calls.
use super::lower::{lower_program, TargetLayout};
use super::*;
use crate::type_checker::{ClassTable, TypedProgram};
use std::cell::RefCell;
use std::rc::Rc;

fn checked(source: &str, target: TargetLayout) -> (TypedProgram, ClassTable, CheckedIr) {
    let tokens = crate::lexer::tokenize(source).unwrap();
    let ast = crate::parser::parse_program(&tokens).unwrap();
    let (typed, classes) = crate::type_checker::check_program(ast).unwrap();
    let ir = lower_program(&typed, &classes, target).unwrap();
    (typed, classes, ir)
}

struct Machine<'a> {
    program: &'a ProgramIr,
    target: TargetLayout,
    memory: Vec<u8>,
    addresses: Vec<usize>,
    input: Vec<u8>,
    cursor: usize,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    trace: Vec<String>,
    steps: usize,
}

impl<'a> Machine<'a> {
    fn new(program: &'a ProgramIr, target: TargetLayout, input: &str) -> Self {
        let mut m = Self {
            program,
            target,
            memory: vec![0; 16],
            addresses: vec![0; program.symbols.len()],
            input: input.as_bytes().to_vec(),
            cursor: 0,
            stdout: vec![],
            stderr: vec![],
            trace: vec![],
            steps: 100_000,
        };
        // Every symbol has a distinct address, including external functions.
        for index in 0..program.symbols.len() {
            m.addresses[index] = m.allocate(8, 8);
        }
        for data in &program.data {
            let size = data
                .items
                .iter()
                .map(|item| match item {
                    DataItem::U32(_) => 4,
                    DataItem::Addr(_) => target.pointer_bytes() as usize,
                    DataItem::Bytes(b) => b.len(),
                    DataItem::Zero(n) => *n as usize,
                })
                .sum();
            m.addresses[data.symbol.0] = m.allocate(size, data.align as usize);
        }
        for data in &program.data {
            let mut address = m.addresses[data.symbol.0];
            for item in &data.items {
                match item {
                    DataItem::U32(n) => {
                        m.write(address, 4, i64::from(*n));
                        address += 4;
                    }
                    DataItem::Addr(id) => {
                        m.write(
                            address,
                            target.pointer_bytes() as usize,
                            m.addresses[id.0] as i64,
                        );
                        address += target.pointer_bytes() as usize;
                    }
                    DataItem::Bytes(b) => {
                        m.memory[address..address + b.len()].copy_from_slice(b);
                        address += b.len();
                    }
                    DataItem::Zero(n) => address += *n as usize,
                }
            }
        }
        let empty = m.string(&[]);
        let id = program
            .symbols
            .iter()
            .position(|s| s.name == "LO_EMPTY_STRING")
            .unwrap();
        m.addresses[id] = empty as usize;
        m
    }

    fn allocate(&mut self, size: usize, alignment: usize) -> usize {
        let base = (self.memory.len() + alignment - 1) & !(alignment - 1);
        self.memory.resize(base + size.max(1), 0);
        base
    }

    // Access widths are independent of field slots. Allocation takes instance_size
    // from the descriptor, leaving the unused upper half of native scalar slots zero.
    fn width(&self, ty: IrType) -> usize {
        match ty {
            IrType::Int32 | IrType::Bool => 4,
            _ => self.target.pointer_bytes() as usize,
        }
    }

    fn write(&mut self, address: usize, width: usize, value: i64) {
        self.memory[address..address + width].copy_from_slice(&value.to_le_bytes()[..width]);
    }

    fn read(&self, address: usize, ty: IrType) -> i64 {
        let width = self.width(ty);
        let mut bytes = [0; 8];
        bytes[..width].copy_from_slice(&self.memory[address..address + width]);
        let value = i64::from_le_bytes(bytes);
        if ty == IrType::Int32 {
            i64::from(value as i32)
        } else {
            value
        }
    }

    fn operand(&self, op: Operand, values: &[i64]) -> i64 {
        match op {
            Operand::Value(id) => values[id.0],
            Operand::Int(n) => i64::from(n),
            Operand::Bool(b) => i64::from(b),
            Operand::Null => 0,
            Operand::Symbol(id) => self.addresses[id.0] as i64,
        }
    }

    fn run(&mut self) -> Result<i32, i32> {
        self.call(self.program.startup.unwrap(), &[], 0)
            .map(|r| r.unwrap() as i32)
    }

    fn call(&mut self, symbol: SymbolId, args: &[i64], depth: usize) -> Result<Option<i64>, i32> {
        assert!(depth < 128, "test call stack overflow");
        let name = &self.program.symbols[symbol.0].name;
        self.trace.push(name.clone());
        let Some(function) = self.program.functions.iter().find(|f| f.symbol == symbol) else {
            return self.runtime(name, args);
        };
        let mut values = vec![0; function.register_types.len()];
        assert_eq!(args.len(), function.params.len());
        for (&id, &value) in function.params.iter().zip(args) {
            values[id.0] = value;
        }
        let ptr = self.target.pointer_bytes() as usize;
        let roots = self.allocate(function.root_slots as usize * ptr, ptr);
        let mut block = function.entry;
        loop {
            self.steps = self
                .steps
                .checked_sub(1)
                .expect("test CFG did not terminate");
            let body = &function.blocks[block.0];
            for instruction in &body.instructions {
                let read = |op| self.operand(op, &values);
                let result = match &instruction.kind {
                    InstructionKind::Copy { src, .. } => Some(read(*src)),
                    InstructionKind::Unary { op, src, .. } => Some(match op {
                        UnaryOp::Neg => i64::from((read(*src) as i32).wrapping_neg()),
                        UnaryOp::Not => i64::from(read(*src) == 0),
                    }),
                    InstructionKind::Binary { op, lhs, rhs, .. } => {
                        let (a, b) = (read(*lhs), read(*rhs));
                        Some(match op {
                            BinaryOp::Add => i64::from((a as i32).wrapping_add(b as i32)),
                            BinaryOp::Sub => i64::from((a as i32).wrapping_sub(b as i32)),
                            BinaryOp::Mul => i64::from((a as i32).wrapping_mul(b as i32)),
                            BinaryOp::Div => {
                                assert!(b != 0 && b != -1);
                                i64::from(a as i32 / b as i32)
                            }
                            BinaryOp::Mod => {
                                assert!(b != 0 && b != -1);
                                i64::from(a as i32 % b as i32)
                            }
                            BinaryOp::Eq => i64::from(a == b),
                            BinaryOp::Lt => i64::from(a < b),
                            BinaryOp::Gt => i64::from(a > b),
                        })
                    }
                    InstructionKind::Load { dst, base, offset } => Some(self.read(
                        (read(*base) + i64::from(*offset)) as usize,
                        function.register_types[dst.0],
                    )),
                    InstructionKind::Store {
                        base,
                        offset,
                        value,
                        ty,
                    } => {
                        let address = (read(*base) + i64::from(*offset)) as usize;
                        let value = read(*value);
                        self.write(address, self.width(*ty), value);
                        None
                    }
                    InstructionKind::RootStore { slot, value } => {
                        let value = read(*value);
                        self.write(roots + *slot as usize * ptr, ptr, value);
                        None
                    }
                    InstructionKind::RootLoad { slot, .. } => {
                        Some(self.read(roots + *slot as usize * ptr, IrType::Ref))
                    }
                    InstructionKind::Call { target, args, .. } => {
                        let symbol = match target {
                            CallTarget::Direct(id) => *id,
                            CallTarget::Indirect(op) => SymbolId(
                                self.addresses
                                    .iter()
                                    .position(|&a| a == read(*op) as usize)
                                    .expect("invalid code pointer"),
                            ),
                        };
                        let args = args.iter().map(|op| read(*op)).collect::<Vec<_>>();
                        self.call(symbol, &args, depth + 1)?
                    }
                };
                if let Some(dst) = instruction.kind.destination() {
                    values[dst.0] = result.expect("instruction result");
                }
            }
            block = match &body.terminator {
                Terminator::Jump(b) => *b,
                Terminator::Branch {
                    condition,
                    then_block,
                    else_block,
                } => {
                    if self.operand(*condition, &values) != 0 {
                        *then_block
                    } else {
                        *else_block
                    }
                }
                Terminator::Return(value) => return Ok(value.map(|op| self.operand(op, &values))),
                Terminator::Abort { target, args } => {
                    let args = args
                        .iter()
                        .map(|op| self.operand(*op, &values))
                        .collect::<Vec<_>>();
                    self.call(*target, &args, depth + 1)?;
                    panic!("abort helper returned");
                }
            };
        }
    }

    fn string(&mut self, bytes: &[u8]) -> i64 {
        let header = self.target.object_header_bytes() as usize;
        let address = self.allocate(
            header + 4 + bytes.len(),
            self.target.pointer_bytes() as usize,
        );
        self.write(address + header, 4, bytes.len() as i64);
        self.memory[address + header + 4..address + header + 4 + bytes.len()]
            .copy_from_slice(bytes);
        address as i64
    }

    fn string_bytes(&self, address: i64) -> &[u8] {
        let base = address as usize + self.target.object_header_bytes() as usize;
        let length = self.read(base, IrType::Int32) as usize;
        &self.memory[base + 4..base + 4 + length]
    }

    fn is_instance(&self, object: i64, target: i64) -> bool {
        if object == 0 {
            return false;
        }
        let mut descriptor = self.read(object as usize, IrType::Ptr);
        let parent_offset = if self.target == TargetLayout::Wasm32 {
            8
        } else {
            16
        };
        while descriptor != 0 {
            if descriptor == target {
                return true;
            }
            descriptor = self.read(descriptor as usize + parent_offset, IrType::Ptr);
        }
        false
    }

    fn token(&mut self) -> &[u8] {
        while self.cursor < self.input.len() && self.input[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
        let start = self.cursor;
        while self.cursor < self.input.len() && !self.input[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
        &self.input[start..self.cursor]
    }

    fn runtime(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, i32> {
        let result = match name {
            "lo_runtime_init" | "lo_push_frame" | "lo_pop_frame" => None,
            "lo_alloc" => {
                let size_offset = if self.target == TargetLayout::Wasm32 {
                    12
                } else {
                    24
                };
                let size = self.read(args[0] as usize + size_offset, IrType::Int32) as usize;
                let object = self.allocate(size, self.target.pointer_bytes() as usize);
                self.write(object, self.target.pointer_bytes() as usize, args[0]);
                Some(object as i64)
            }
            "lo_gc_write_barrier" => {
                self.write(
                    (args[0] + args[1]) as usize,
                    self.target.pointer_bytes() as usize,
                    args[2],
                );
                None
            }
            "lo_string_new" => {
                let bytes = self.memory[args[0] as usize..(args[0] + args[1]) as usize].to_vec();
                Some(self.string(&bytes))
            }
            "lo_string_concat" => {
                let bytes = [self.string_bytes(args[0]), self.string_bytes(args[1])].concat();
                Some(self.string(&bytes))
            }
            "lo_string_repeat" => {
                if args[1] < 0 {
                    return Err(120);
                }
                let bytes = self.string_bytes(args[0]).repeat(args[1] as usize);
                Some(self.string(&bytes))
            }
            "lo_string_reverse" => {
                let bytes = std::str::from_utf8(self.string_bytes(args[0]))
                    .unwrap()
                    .chars()
                    .rev()
                    .collect::<String>();
                Some(self.string(bytes.as_bytes()))
            }
            "lo_string_compare" => Some(
                match self.string_bytes(args[0]).cmp(self.string_bytes(args[1])) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                },
            ),
            "lo_cast_check" => {
                if args[0] != 0 && !self.is_instance(args[0], args[1]) {
                    return Err(101);
                }
                Some(args[0])
            }
            "lo_instanceof" => Some(i64::from(self.is_instance(args[0], args[1]))),
            "lo_abort_null_receiver" => {
                assert!(!self.memory[args[0] as usize..(args[0] + args[1]) as usize].is_empty());
                return Err(102);
            }
            "lo_print_int" | "lo_print_bool" | "lo_print_string" | "lo_println" => {
                let bytes = match name {
                    "lo_print_int" => (args[0] as i32).to_string().into_bytes(),
                    "lo_print_bool" => {
                        if args[0] != 0 {
                            b"true".to_vec()
                        } else {
                            b"false".to_vec()
                        }
                    }
                    "lo_print_string" => self.string_bytes(args[0]).to_vec(),
                    _ => b"\n".to_vec(),
                };
                if *args.last().unwrap() == 0 {
                    self.stdout.extend(bytes);
                } else {
                    self.stderr.extend(bytes);
                }
                None
            }
            "lo_read_int" => {
                let token = self.token();
                if token.is_empty() {
                    return Err(111);
                }
                Some(i64::from(
                    std::str::from_utf8(token)
                        .unwrap()
                        .parse::<i32>()
                        .map_err(|_| 110)?,
                ))
            }
            "lo_read_bool" => Some(match self.token() {
                b"true" => 1,
                b"false" => 0,
                _ => return Err(112),
            }),
            "lo_read_string" => {
                let start = self.cursor;
                while self.cursor < self.input.len() && self.input[self.cursor] != b'\n' {
                    self.cursor += 1;
                }
                let bytes = self.input[start..self.cursor].to_vec();
                if self.cursor < self.input.len() {
                    self.cursor += 1;
                }
                Some(self.string(&bytes))
            }
            "lo_eof" => Some(i64::from(self.cursor == self.input.len())),
            _ => panic!("unexpected runtime call {name}"),
        };
        Ok(result)
    }
}

#[derive(Clone)]
struct Buffer(Rc<RefCell<Vec<u8>>>);
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn compare(source: &str, input: &str, expected: Result<i32, i32>, stdout: &str, stderr: &str) {
    compare_impl(source, input, expected, stdout, stderr, true);
}

// The AST interpreter rejects prebound writes internally and null-checks before
// actuals, unlike the existing WASM backend and the agreed lowering contract.
fn compare_ir(source: &str, input: &str, expected: Result<i32, i32>, stdout: &str, stderr: &str) {
    compare_impl(source, input, expected, stdout, stderr, false);
}

fn compare_impl(
    source: &str,
    input: &str,
    expected: Result<i32, i32>,
    stdout: &str,
    stderr: &str,
    check_ast: bool,
) {
    for target in [TargetLayout::Wasm32, TargetLayout::X86_64] {
        let (typed, classes, ir) = checked(source, target);
        let mut machine = Machine::new(ir.program(), target, input);
        assert_eq!(machine.run(), expected, "{target:?}: {source}");
        assert_eq!(machine.stdout, stdout.as_bytes(), "{target:?}");
        assert_eq!(machine.stderr, stderr.as_bytes(), "{target:?}");
        assert_eq!(machine.trace[1], "lo_runtime_init");
        assert_eq!(machine.trace[2], "lo_push_frame");
        if expected.is_ok() {
            assert_eq!(machine.trace.last().unwrap(), "lo_pop_frame");
        }
        if !check_ast {
            continue;
        }
        let out = Buffer(Rc::default());
        let err = Buffer(Rc::default());
        let io = crate::interpreter::Io::new(
            Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
            Box::new(out.clone()),
            Box::new(err.clone()),
        );
        let outcome = crate::interpreter::interpret(
            &typed,
            &classes,
            crate::interpreter::DEFAULT_HEAP_LIMIT,
            io,
        );
        let actual = match outcome {
            crate::interpreter::Outcome::Exit(n) => Ok(n),
            crate::interpreter::Outcome::Abort(a) => Err(a.code()),
        };
        assert_eq!(actual, expected, "AST interpreter: {source}");
        assert_eq!(*out.0.borrow(), stdout.as_bytes());
        assert_eq!(*err.0.borrow(), stderr.as_bytes());
    }
}

#[test]
fn strings_use_content_semantics_and_utf8() {
    compare(
        r#"class Main () { int main() { String s; out.print_string(s); s = ("é🙂" + "x"); out.print_string((~ s)); out.print_string(("a" * 3)); out.print_bool((s = "é🙂x")); out.print_bool(("a" < "b")); out.print_bool(("b" > "a")); out.println(); return (("" = s) ? 1 : 0); } }"#,
        "",
        Ok(0),
        "x🙂éaaatruetruetrue\n",
        "",
    );
    compare(
        r#"class Main () { int main() { String s; s = (true ? "same" : "other"); return ((s = ("sa" + "me")) ? 7 : 0); } }"#,
        "",
        Ok(7),
        "",
        "",
    );
    compare(
        r#"class Main () { int main() { out.print_string(("a" * (~ 1))); return 0; } }"#,
        "",
        Err(120),
        "",
        "",
    );
}

#[test]
fn fields_inheritance_constructors_and_dispatch() {
    compare(
        r#"
        class Parent (int number; String text;) [ Parent(int n) { number = n; out.print_string(text); } ] {
            int value() { return number; }
            String label() { return text; }
        }
        class Child extends Parent (bool flag; Parent link;) [ Child() { this(9); } Child(int n) { super(n); flag = true; link = this; text = "child"; } ] {
            int value() { return (super.value() + (flag ? 1 : 0)); }
            Parent identity() { return link; }
        }
        class Main () { int main() { Parent p; Child c; c = new Child(); p = c.identity(); out.print_string(p.label()); return p.value(); } }
    "#,
        "",
        Ok(10),
        "child",
        "",
    );
    compare(
        r#"class Box (int n; String s;) { int value() { out.print_string(s); return n; } } class Main () { int main() { Box b; b = new Box(12, "implicit"); return b.value(); } }"#,
        "",
        Ok(12),
        "implicit",
        "",
    );
}

#[test]
fn calls_preserve_order_snapshots_recursion_and_short_circuit() {
    compare(
        r#"
        class Main (int state;) [ Main() { state = 1; } ] {
            int mutate() { state = 9; out.print_int(2); return 3; }
            int combine(int a, int b) { return ((a * 10) + b); }
            Main receiver() { out.print_int(1); return this; }
            int factorial(int n) { if ((n < 2)) { return 1; } else { return (n * this.factorial((n - 1))); } }
            int main() { int x; x = (this.receiver()).combine(state, this.mutate()); if ((false & (this.mutate() = 0))) { return 0; } else { ; } return (x + this.factorial(4)); }
        }
    "#,
        "",
        Ok(37),
        "12",
        "",
    );
}

#[test]
fn casts_instanceof_and_abort_paths() {
    let prefix = "class Animal () { int value() { return 1; } } class Dog extends Animal () [ Dog() { super(); } ] { int value() { return 2; } } class Cat extends Animal () [ Cat() { super(); } ] { int value() { return 3; } } ";
    compare(&format!("{prefix} class Main () {{ int main() {{ Animal a; Dog d; a = new Dog(); d = ((Dog) a); a = ((Animal) d); return ((a instanceof Dog) ? d.value() : 0); }} }}"), "", Ok(2), "", "");
    compare(&format!("{prefix} class Main () {{ int main() {{ Animal a; Dog d; d = ((Dog) a); return ((d instanceof Dog) ? 1 : 0); }} }}"), "", Ok(0), "", "");
    compare(&format!("{prefix} class Main () {{ int main() {{ Animal a; Dog d; a = new Cat(); d = ((Dog) a); return 0; }} }}"), "", Err(101), "", "");
    compare(
        &format!("{prefix} class Main () {{ int main() {{ Animal a; return a.value(); }} }}"),
        "",
        Err(102),
        "",
        "",
    );
    compare_ir("class A () { int f(int n) { return n; } } class Main () { int argument() { out.print_int(5); return 0; } int main() { A a; return a.f(this.argument()); } }", "", Err(102), "5", "");
}

#[test]
fn prebound_reassignment_and_io_wrappers() {
    compare_ir("class Main () { int main() { Output saved; saved = out; out = err; out.print_int(1); saved.print_int(2); err = saved; err.print_bool(true); out.println(); return 0; } }", "", Ok(0), "2true", "1\n");
    compare("class Main () { int main() { int n; bool b; n = in.read_int(); b = in.read_bool(); out.print_int(n); err.print_bool(b); out.println(); return (in.eof() ? 1 : 0); } }", "41 true", Ok(1), "41\n", "true");
    compare("class Main () { int main() { String s; while ((! in.eof())) { s = in.read_string(); out.print_string(s); } return 0; } }", "hello\né🙂\n", Ok(0), "helloé🙂", "");
    compare_ir("class Main () { int main() { Input saved; saved = in; in = null; return saved.read_int(); } }", "8", Ok(8), "", "");
    compare_ir(
        "class Main () { int main() { out = null; out.println(); return 0; } }",
        "",
        Err(102),
        "",
        "",
    );
    compare(
        "class Main () { int main() { return in.read_int(); } }",
        "",
        Err(111),
        "",
        "",
    );
    compare(
        "class Main () { int main() { return in.read_int(); } }",
        "bad",
        Err(110),
        "",
        "",
    );
    compare(
        "class Main () { int main() { return (in.read_bool() ? 1 : 0); } }",
        "bad",
        Err(112),
        "",
        "",
    );
}

#[test]
fn target_layout_metadata_and_inherited_slots() {
    let source = r#"
        class Child extends Parent (Parent link; int tail;) [ Child() { super(0, false, ""); } ] {
            int value() { return 2; }
            int extra() { return tail; }
        }
        class Parent (int count; bool flag; String text;) { int value() { return count; } }
        class Main () { int main() { return 0; } }
    "#;
    for target in [TargetLayout::Wasm32, TargetLayout::X86_64] {
        let (_, _, checked) = checked(source, target);
        let ir = checked.program();
        let machine = Machine::new(ir, target, "");
        let address =
            |name: &str| machine.addresses[ir.symbols.iter().position(|s| s.name == name).unwrap()];
        let child = address("lo_class_5_Child");
        let parent = address("lo_class_6_Parent");
        let ptr = target.pointer_bytes() as usize;
        let (
            parent_offset,
            size_offset,
            refs_offset,
            count_offset,
            slots_offset,
            parent_size,
            child_size,
            text,
            link,
        ) = match target {
            TargetLayout::Wasm32 => (8, 12, 16, 20, 24, 24, 32, 20, 24),
            TargetLayout::X86_64 => (16, 24, 32, 40, 44, 40, 56, 32, 40),
        };
        assert_eq!(
            machine.read(child + parent_offset, IrType::Ptr),
            parent as i64
        );
        assert_eq!(
            machine.read(parent + size_offset, IrType::Int32),
            parent_size
        );
        assert_eq!(machine.read(child + size_offset, IrType::Int32), child_size);
        assert_eq!(machine.read(child + count_offset, IrType::Int32), 2);
        let refs = machine.read(child + refs_offset, IrType::Ptr) as usize;
        assert_eq!(machine.read(refs, IrType::Int32), text);
        assert_eq!(machine.read(refs + 4, IrType::Int32), link);
        assert_eq!(machine.read(child + slots_offset, IrType::Int32), 2);
        let table = machine.read(
            child + target.descriptor_vtable_offset() as usize,
            IrType::Ptr,
        ) as usize;
        assert_eq!(
            machine.read(table, IrType::Ptr),
            address("lo_method_5_Child_5_value") as i64
        );
        assert_eq!(
            machine.read(table + ptr, IrType::Ptr),
            address("lo_method_5_Child_5_extra") as i64
        );
        let descriptor = ir
            .data
            .iter()
            .find(|d| ir.symbols[d.symbol.0].name == "lo_class_5_Child")
            .unwrap();
        let size: usize = descriptor
            .items
            .iter()
            .map(|d| match d {
                DataItem::Addr(_) => ptr,
                DataItem::U32(_) => 4,
                DataItem::Zero(n) => *n as usize,
                DataItem::Bytes(b) => b.len(),
            })
            .sum();
        assert_eq!(
            size,
            if target == TargetLayout::Wasm32 {
                32
            } else {
                56
            }
        );
        let frame = crate::layout::Target {
            ptr: target.pointer_bytes(),
        }
        .frame();
        let id = ir.find_symbol("lo_bindings").unwrap();
        let data = ir.data.iter().find(|d| d.symbol == id).unwrap();
        assert_eq!(data.align as usize, ptr);
        assert_eq!(data.section, Section::Writable);
        let address = machine.addresses[id.0];
        assert_eq!(machine.read(address, IrType::Ptr), 0);
        assert_eq!(
            machine.read(address + frame.num_roots as usize, IrType::Int32),
            3
        );
        for slot in 0..3 {
            assert_eq!(
                machine.read(address + frame.root(slot) as usize, IrType::Ref),
                0
            );
        }
        let size: usize = data
            .items
            .iter()
            .map(|item| match item {
                DataItem::Zero(n) => *n as usize,
                DataItem::U32(_) => 4,
                _ => panic!("unexpected static frame item"),
            })
            .sum();
        assert_eq!(size, frame.size(3) as usize);
        assert_eq!(
            size,
            if target == TargetLayout::Wasm32 {
                20
            } else {
                40
            }
        );
        assert!(!ir.symbols.iter().any(|s| s.name.starts_with("lo_binding_")));
    }
}

#[test]
fn allocation_evaluates_actuals_before_constructor_and_defaults() {
    compare(
        r#"
        class Box (String text;) [ Box(int n) { out.print_int(n); out.print_string(text); } ] { int value() { return 4; } }
        class Main () {
            int argument() { out.print_int(1); return 2; }
            int main() { Box b; b = new Box(this.argument()); return b.value(); }
        }
    "#,
        "",
        Ok(4),
        "12",
        "",
    );
    let (_, _, ir) = checked(
        "class Main () { int main() { return 0; } }",
        TargetLayout::Wasm32,
    );
    let startup = ir
        .program()
        .functions
        .iter()
        .find(|f| Some(f.symbol) == ir.program().startup)
        .unwrap();
    assert_eq!(startup.root_slots, 0);
    let instructions: Vec<_> = startup
        .blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .collect();
    let frame = ir.program().find_symbol("lo_bindings").unwrap();
    let stores: Vec<_> = instructions
        .iter()
        .filter_map(|i| match i.kind {
            InstructionKind::Store {
                base: Operand::Symbol(id),
                offset,
                ty: IrType::Ref,
                ..
            } if id == frame => Some(offset),
            _ => None,
        })
        .collect();
    assert_eq!(stores, vec![8, 12, 16]);
    let calls: Vec<_> = instructions
        .iter()
        .filter_map(|i| match &i.kind {
            InstructionKind::Call {
                target: CallTarget::Direct(id),
                args,
                ..
            } => Some((ir.program().symbols[id.0].name.as_str(), args)),
            _ => None,
        })
        .collect();
    assert_eq!(calls[0].0, "lo_runtime_init");
    assert_eq!(calls[1].0, "lo_push_frame");
    assert!(matches!(calls[1].1.as_slice(), [Operand::Symbol(id)] if *id == frame));
    assert_eq!(calls.last().unwrap().0, "lo_pop_frame");
    let first_call = instructions
        .iter()
        .find_map(|i| match i.kind {
            InstructionKind::Call {
                target: CallTarget::Direct(id),
                ..
            } => Some(id),
            _ => None,
        })
        .unwrap();
    assert_eq!(ir.program().symbols[first_call.0].name, "lo_runtime_init");
    assert!(ir
        .program()
        .functions
        .iter()
        .filter(|f| Some(f.symbol) != ir.program().startup)
        .all(|f| f.root_slots == 0));
}

#[test]
fn static_bytes_are_interned_and_string_calls_keep_distinct_types() {
    let (_, _, ir) = checked(
        r#"class Main () { int main() { out.print_string("é🙂"); out.print_string("é🙂"); return (("a" < "b") ? 1 : 0); } }"#,
        TargetLayout::X86_64,
    );
    let program = ir.program();
    let literals: Vec<_> = program
        .data
        .iter()
        .filter(
            |d| matches!(d.items.as_slice(), [DataItem::Bytes(bytes)] if bytes == "é🙂".as_bytes()),
        )
        .collect();
    assert_eq!(literals.len(), 1);
    let calls: Vec<_> = program
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.instructions)
        .filter_map(|i| match &i.kind {
            InstructionKind::Call {
                target: CallTarget::Direct(id),
                args,
                ..
            } if program.symbols[id.0].name == "lo_string_new" => Some(args),
            _ => None,
        })
        .collect();
    assert_eq!(calls.iter().filter(|args| matches!(args.as_slice(), [Operand::Symbol(id), Operand::Int(6)] if *id == literals[0].symbol)).count(), 2);
    let reverse = program
        .symbols
        .iter()
        .find(|s| s.name == "lo_string_reverse")
        .unwrap();
    let SymbolKind::Function(sig) = reverse.kind else {
        panic!()
    };
    assert_eq!(
        program.signatures[sig.0],
        Signature {
            params: vec![IrType::Ref],
            result: Some(IrType::Ref)
        }
    );
}

#[test]
fn scalar_fields_use_pointer_sized_slots_and_four_byte_accesses() {
    let source = "class Parent (int n;) { int value() { return n; } } class Child extends Parent (int m;) [ Child() { super(1); m = 2; } ] { int sum() { return (n + m); } } class Main () { int main() { Child c; c = new Child(); return c.sum(); } }";
    compare(source, "", Ok(3), "", "");
    for target in [TargetLayout::Wasm32, TargetLayout::X86_64] {
        let (_, _, ir) = checked(source, target);
        let expected = target.object_header_bytes() + target.pointer_bytes();
        assert!(ir
            .program()
            .dump()
            .contains(&format!("store Int32 [this + {expected}], 2")));
    }
    compare("class Pair (int a; int b;) [ Pair() { a = 1; b = 2; } ] { int sum() { return (a + b); } } class Main () { int main() { Pair p; p = new Pair(); return p.sum(); } }", "", Ok(3), "", "");
}

#[test]
fn shared_smoke_fixtures_match_interpreter_on_both_targets() {
    compare(
        include_str!("../../../tests/lo_programs/alloc_basic.lo"),
        "",
        Ok(0),
        include_str!("../../../tests/expected/alloc_basic.out"),
        "",
    );
    compare(
        include_str!("../../../tests/lo_programs/class_basic.lo"),
        "",
        Ok(0),
        include_str!("../../../tests/expected/class_basic.out"),
        "",
    );
    compare(
        include_str!("../../../tests/lo_programs/string_basic.lo"),
        "",
        Ok(0),
        include_str!("../../../tests/expected/string_basic.out"),
        "",
    );
}

#[test]
fn receiver_snapshot_survives_prebound_changes_in_arguments() {
    compare_ir("class Main () { int redirect() { out = err; return 7; } int main() { out.print_int(this.redirect()); out.print_int(8); return 0; } }", "", Ok(0), "7", "8");
}

#[test]
fn upcasts_and_literal_null_casts_emit_no_runtime_check() {
    let source = "class Animal () { int value() { return 1; } } class Dog extends Animal () [ Dog() { super(); } ] { int value() { return 2; } } class Main () { int main() { Animal a; Dog d; d = ((Dog) null); a = ((Animal) d); return ((a instanceof Animal) ? 1 : 0); } }";
    compare(source, "", Ok(0), "", "");
    let (_, _, ir) = checked(source, TargetLayout::Wasm32);
    assert!(!ir.program().functions.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.instructions).any(|i| {
        matches!(i.kind, InstructionKind::Call { target: CallTarget::Direct(id), .. } if ir.program().symbols[id.0].name == "lo_cast_check")
    }));
}

#[test]
fn lowering_layout_matches_shared_class_layout_for_fixtures() {
    let fixtures = [
        include_str!("../../../tests/lo_programs/alloc_basic.lo"),
        include_str!("../../../tests/lo_programs/class_basic.lo"),
        include_str!("../../../tests/lo_programs/string_basic.lo"),
        "class Parent (int a; int b; bool flag; String text;) { int get() { return a; } } class Child extends Parent (Parent link; int n;) [ Child() { super(0, 0, false, \"\"); } ] { int get() { return n; } } class Main () { int main() { return 0; } }",
    ];
    for target in [TargetLayout::X86_64, TargetLayout::Wasm32] {
        let shared_target = crate::layout::Target {
            ptr: target.pointer_bytes(),
        };
        for source in fixtures {
            let (typed, table, ir) = checked(source, target);
            let machine = Machine::new(ir.program(), target, "");
            for class in &typed.classes {
                let expected =
                    crate::layout::class_layout(shared_target, &table, &class.class_name);
                let (fields, size) =
                    lower::class_layout_for_test(&table, target, &class.class_name).unwrap();
                assert_eq!(
                    size, expected.instance_size,
                    "{target:?}: {}",
                    class.class_name
                );
                assert_eq!(fields.len(), expected.fields.len());
                for field in &expected.fields {
                    assert_eq!(
                        fields[&field.name] as u32, field.offset,
                        "{target:?}: {}.{}",
                        class.class_name, field.name
                    );
                }
                let name = crate::symbols::class_descriptor(&class.class_name);
                let id = ir.program().find_symbol(&name).unwrap();
                let descriptor = machine.addresses[id.0];
                let abi = shared_target.descriptor();
                assert_eq!(
                    machine.read(descriptor + abi.instance_size as usize, IrType::Int32) as u32,
                    expected.instance_size
                );
                let refs =
                    machine.read(descriptor + abi.pointer_offsets as usize, IrType::Ptr) as usize;
                let actual_refs = (0..expected.pointer_offsets.len())
                    .map(|i| machine.read(refs + i * 4, IrType::Int32) as u32)
                    .collect::<Vec<_>>();
                assert_eq!(actual_refs, expected.pointer_offsets);
            }
        }
    }
}
