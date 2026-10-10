# Hello world, end to end

This walks one small program through the whole back end: the IR it lowers to, then the assembly that comes out under each allocation strategy. Every listing is real compiler output; the `#` comments were added to explain it.

```
class Main () {
    int main() {
        out.print_string("Hello, world!");
        out.println();
        return 0;
    }
}
```

The IR comes from `lo-compiler --dump-ir hello.lo` and is **identical for both strategies**, because allocation happens after the IR is final. The assembly comes from `lo-compiler --emit-asm hello.lo --spill-all` (every value in its own stack slot) and `lo-compiler --emit-asm hello.lo` (linear scan, the default).

## 1. The IR

### Reading a dump

| Notation | Meaning |
|---|---|
| `extern func @name(...)` | a runtime function: declared here, defined in `liblo_runtime.a` |
| `.rodata @x` / `.data @x` | static data: read-only (descriptors, vtables, bytes) or writable |
| `func @name(params) -> T entry .L0 roots N` | a function; `N` is its count of shadow-stack root slots |
| `t3:Bool = t1 == null` | a three-address instruction: destination, type, operation |
| `tN`, or a source name | virtual registers: mutable, typed, not SSA |
| `load [p + 24]`, `store T [p + 24], v` | memory access at a byte offset; `T` is the access type |
| `call @f(...)`, `call_indirect t(...)` | direct and through-a-pointer calls |
| `cbr c, .L1, .L2` | conditional branch (true, false) |
| `abort @f(...)` | block terminator for a call that never returns |
| `root_store rootN, v` / `root_load rootN` | write and read a shadow-stack slot, inserted around calls so a moving collector can update live references |
| `; line N` | source line |

The dump has three parts, in this order: the `extern` declarations (21 runtime functions, omitted below), the static data, and the functions.

### Static data

`lo_bindings` is a writable frame in the layout of a shadow-stack frame. It holds `in`, `out` and `err` as GC roots (see [the bindings doc](p2-bindings-static-frame.md)):

```
.data @lo_bindings align 8 {
  zero 8                                                      # ShadowFrame.parent (the runtime fills it in)
  u32 3                                                       # num_roots = 3: in, out, err
  zero 4                                                      # padding
  zero 24                                                     # roots[0..2], null until lo_entry fills them
}
```

`Main`'s class descriptor, vtable and the string literal's bytes:

```
.rodata @lo_class_4_Main align 8 {
  addr @lo_bytes_2                                            # name -> the bytes "Main\0"
  u32 4                                                       # name_len
  zero 4                                                      # padding to 8-byte alignment
  zero 8                                                      # parent: null (Main has no superclass)
  u32 16                                                      # instance_size: the 16-byte header, no fields
  zero 4                                                      # padding to 8-byte alignment
  addr @lo_class_4_Main_pointers                              # table of GC pointer offsets
  u32 0                                                       # pointer_count
  u32 1                                                       # vtable_size
  addr @lo_class_4_Main_vtable                                # vtable
}
.rodata @lo_class_4_Main_vtable align 8 {
  addr @lo_method_4_Main_4_main                               # slot 0: Main.main
}
.rodata @lo_bytes_3 align 1 {
  bytes 48 65 6c 6c 6f 2c 20 77 6f 72 6c 64 21                # "Hello, world!" (13 bytes, no terminator)
}
```

### `Output.print_string`, a lowered I/O wrapper

```
func @lo_method_6_Output_12_print_string(this:Ref, s:Ref) -> Void entry .L0 roots 0 {
.L0:
  t2:Int32 = load [this + 16] ; line 0                        # an Output's hidden field at +16: 0 = stdout, 1 = stderr
  call @lo_print_string(s, t2) ; line 0                       # runtime call: (string, destination)
  ret
}
```

### `Main.main`

```
func @lo_method_4_Main_4_main(this:Ref) -> Int32 entry .L0 roots 1 {  # symbol = lo_method_<len>_<class>_<len>_<method>; `roots 1` = one shadow-stack slot
.L0:
  t1:Ref = load [@lo_bindings + 24] ; line 6                  # `out`: slot 1 of the static bindings frame (16 + 8*1 = 24)
  root_store root0, t1 ; line 6                               # park it in the shadow stack: the next call may collect and move objects
  t2:Ref = call @lo_string_new(@lo_bytes_3, 13) ; line 6      # build the String object for the literal (13 UTF-8 bytes)
  t1:Ref = root_load root0 ; line 6                           # reload `out`; the collector may have rewritten the slot
  t3:Bool = t1 == null ; line 6                               # null-receiver check before calling print_string
  cbr t3, .L1, .L2                                            # conditional branch: true -> .L1 (abort), false -> .L2
.L1:
  abort @lo_abort_null_receiver(@lo_bytes_4, 12)              # terminator: print the message, exit 102; the call never returns
.L2:
  root_store root0, null ; line 6                             # done with the root: null it so the collector keeps nothing alive
  call @lo_method_6_Output_12_print_string(t1, t2) ; line 6   # I/O methods are called directly, not through the vtable
  t4:Ref = load [@lo_bindings + 24] ; line 7                  # `out` again, for println
  t5:Bool = t4 == null ; line 7                               # second null-receiver check
  cbr t5, .L3, .L4                                            # same shape: true -> .L3 (abort), false -> .L4
.L3:
  abort @lo_abort_null_receiver(@lo_bytes_5, 7)               # exit 102: cannot dispatch println
.L4:
  root_store root0, null ; line 7                             # null the root again
  call @lo_method_6_Output_7_println(t4) ; line 7             # second call, same receiver
  ret 0                                                       # return the int; this becomes the process exit status
}
```

`Main.main` has two nearly identical halves, one per call: fetch `out`, check it for null (aborting with exit 102 if it is), park it in the root slot across anything that could collect, call, then null the slot. Only the first half needs the park-and-reload around `lo_string_new`, because that call can trigger a collection while `out` is live.

### `lo_entry`, the startup function

The IR contains runs of `root_store rootN, null` before every call (the GC-root pass nulls slots it owns); they are omitted here.

```
func @lo_entry() -> Int32 entry .L0 roots 4 startup {         # the startup function; process `main` calls it
.L0:
  call @lo_runtime_init() ; line 0                            # must run first; the runtime resets its frame chain here
  call @lo_push_frame(@lo_bindings) ; line 0                  # link the static frame that holds in/out/err as GC roots
  t0:Ref = call @lo_alloc(@lo_class_5_Input) ; line 0         # allocate the stdin object ...
  root_store root0, t0 ; line 0
  call @lo_ctor_5_Input_0(t0) ; line 0
  t0:Ref = root_load root0 ; line 0
  store Ref [@lo_bindings + 16], t0 ; line 0                  # ... and publish it as `in` (a static root: no write barrier)
  t1:Ref = call @lo_alloc(@lo_class_6_Output) ; line 0
  root_store root1, t1 ; line 0
  call @lo_ctor_6_Output_0(t1) ; line 0
  t1:Ref = root_load root1 ; line 0
  store Int32 [t1 + 16], 0 ; line 0                           # Output #1 selects destination 0 (stdout)
  store Ref [@lo_bindings + 24], t1 ; line 0                  # published as `out`
  t2:Ref = call @lo_alloc(@lo_class_6_Output) ; line 0
  root_store root2, t2 ; line 0
  call @lo_ctor_6_Output_0(t2) ; line 0
  t2:Ref = root_load root2 ; line 0
  store Int32 [t2 + 16], 1 ; line 0                           # Output #2 selects destination 1 (stderr)
  store Ref [@lo_bindings + 32], t2 ; line 0                  # published as `err`
  t3:Ref = call @lo_alloc(@lo_class_4_Main) ; line 0          # allocate Main and run its constructor ...
  root_store root3, t3 ; line 0
  call @lo_ctor_4_Main_0(t3) ; line 0
  t3:Ref = root_load root3 ; line 0
  t4:Bool = t3 == null ; line 0                               # (null check on a just-allocated object; lowering emits it uniformly)
  cbr t4, .L1, .L2
.L1:
  abort @lo_abort_null_receiver(@lo_bytes_6, 4)
.L2:
  t5:Ptr = load [t3 + 0] ; line 0                             # object -> class descriptor
  t6:Ptr = load [t5 + 48] ; line 0                            # descriptor -> vtable (offset 48)
  t7:CodePtr(Ref) -> Int32 = load [t6 + 0] ; line 0           # vtable -> slot 0: Main.main
  t8:Int32 = call_indirect t7(t3) ; line 0                    # virtual call of Main.main(this)
  call @lo_pop_frame() ; line 0                               # unlink the bindings frame
  ret t8                                                      # Main.main's result is lo_entry's result
}
```

## 2. Assembly with spill-everything

Every virtual register has its own 8-byte stack slot, so each IR instruction loads its operands from the stack, computes in `rax` / `r10` / `r11`, and stores the result back.

Frame of `Main.main` (80 bytes below the saved `rbp`):

```
[rbp - 8 ]  shadow root0          ┐
[rbp - 16]  shadow num_roots      │ the shadow frame the runtime walks
[rbp - 24]  shadow parent         ┘
[rbp - 32]  this      (v0)
[rbp - 40]  t1        (v1)
[rbp - 48]  t2
[rbp - 56]  t3
[rbp - 64]  t4
[rbp - 72]  t5
[rbp - 80]  ret_save  (return value parked across lo_pop_frame)
```

```
lo_method_4_Main_4_main:
    push rbp                                  # save the caller's frame pointer
    mov rbp, rsp                              # rbp = this frame's base
    sub rsp, 80                               # reserve 80 bytes: shadow frame 24 + 6 spill slots 48 + ret_save 8
    mov qword ptr [rbp - 32], rdi             # `this` (rdi) -> its spill slot
    mov qword ptr [rbp - 24], 0               # shadow frame: parent = 0
    mov qword ptr [rbp - 16], 1               # shadow frame: num_roots = 1
    mov qword ptr [rbp - 8], 0                # shadow frame: root0 = null
    lea rdi, [rbp - 24]                       # rdi = &shadow frame
    call lo_push_frame                        # link it into the runtime's frame chain
.Llo_method_4_Main_4_main_0:                  # IR block .L0
    lea r10, [rip + lo_bindings]              # t1 = load [@lo_bindings + 24]:
    mov rax, qword ptr [r10 + 24]             #   fetch the `out` object ...
    mov qword ptr [rbp - 40], rax             #   ... into t1's slot
    mov rax, qword ptr [rbp - 40]             # root_store root0, t1:
    mov qword ptr [rbp - 8], rax              #   park t1 in the shadow-stack slot
    lea rdi, [rip + lo_bytes_3]               # t2 = call lo_string_new: arg 1 = the bytes
    mov esi, 13                               #   arg 2 = 13
    call lo_string_new                        #   call
    mov qword ptr [rbp - 48], rax             #   result -> t2's slot
    mov rax, qword ptr [rbp - 8]              # root_load root0:
    mov qword ptr [rbp - 40], rax             #   reload t1 (GC may have moved it)
    mov rax, qword ptr [rbp - 40]             # t3 = (t1 == null): load t1
    cmp rax, 0                                #   compare with null
    sete al                                   #   al = (t1 == 0)
    movzx eax, al                             #   widen to a 32-bit Bool
    mov dword ptr [rbp - 56], eax             #   store t3
    cmp dword ptr [rbp - 56], 0               # cbr t3: test it
    je .Llo_method_4_Main_4_main_2            #   false -> .L2 (true falls through into .L1)
.Llo_method_4_Main_4_main_1:                  # IR block .L1 (abort)
    lea rdi, [rip + lo_bytes_4]               # arg 1 = method name bytes
    mov esi, 12                               # arg 2 = 12
    call lo_abort_null_receiver               # call; never returns
.Llo_method_4_Main_4_main_2:                  # IR block .L2
    mov qword ptr [rbp - 8], 0                # root_store root0, null
    mov rdi, qword ptr [rbp - 40]             # print_string(t1, t2): arg 1 = receiver
    mov rsi, qword ptr [rbp - 48]             #   arg 2 = the string
    call lo_method_6_Output_12_print_string   #   call
    lea r10, [rip + lo_bindings]              # t4 = load `out` again:
    mov rax, qword ptr [r10 + 24]             #   ...
    mov qword ptr [rbp - 64], rax             #   ... into t4's slot
    mov rax, qword ptr [rbp - 64]             # t5 = (t4 == null)
    cmp rax, 0                                #   ...
    sete al                                   #   ...
    movzx eax, al                             #   ...
    mov dword ptr [rbp - 72], eax             #   ...
    cmp dword ptr [rbp - 72], 0               # cbr t5
    je .Llo_method_4_Main_4_main_4            #   false -> .L4
.Llo_method_4_Main_4_main_3:                  # IR block .L3 (abort)
    lea rdi, [rip + lo_bytes_5]
    mov esi, 7
    call lo_abort_null_receiver
.Llo_method_4_Main_4_main_4:                  # IR block .L4
    mov qword ptr [rbp - 8], 0                # root_store root0, null
    mov rdi, qword ptr [rbp - 64]             # println(t4): arg 1 = receiver
    call lo_method_6_Output_7_println         #   call
    mov eax, 0                                # ret 0: eax = 0
    mov qword ptr [rbp - 80], rax             # park the return value (lo_pop_frame clobbers rax)
    call lo_pop_frame                         # unlink this function's shadow frame
    mov rax, qword ptr [rbp - 80]             # restore the return value
    leave                                     # mov rsp, rbp ; pop rbp
    ret                                       # return
```

## 3. Assembly with linear scan

Values now live in registers. This function calls, so it may only use the callee-saved registers (`rbx`, `r12`–`r15`): they survive the calls. The price is that the prologue must save the ones it uses and the epilogue restore them.

Frame of `Main.main` (80 bytes):

```
[rbp - 8 .. -40]  saved rbx, r12, r13, r14, r15
[rbp - 48]  shadow root0          ┐
[rbp - 56]  shadow num_roots      │ the shadow frame
[rbp - 64]  shadow parent         ┘
[rbp - 72]  ret_save
```

```
lo_method_4_Main_4_main:
    push rbp                                  # save the caller's frame pointer
    mov rbp, rsp                              # rbp = this frame's base
    sub rsp, 80                               # reserve 80 bytes: saved registers 40 + shadow frame 24 + ret_save 8 (+ padding)
    mov qword ptr [rbp - 8], rbx              # save the five callee-saved registers this function will use
    mov qword ptr [rbp - 16], r12
    mov qword ptr [rbp - 24], r13
    mov qword ptr [rbp - 32], r14
    mov qword ptr [rbp - 40], r15             #   (rbx, r12, r13, r14, r15)
    mov rbx, rdi                              # `this`: rdi -> rbx. The parameter now lives in a register
    mov qword ptr [rbp - 64], 0               # shadow frame: parent = 0
    mov qword ptr [rbp - 56], 1               # shadow frame: num_roots = 1
    mov qword ptr [rbp - 48], 0               # shadow frame: root0 = null
    lea rdi, [rbp - 64]                       # rdi = &shadow frame
    call lo_push_frame                        # link it into the runtime's frame chain
.Llo_method_4_Main_4_main_0:                  # IR block .L0
    lea r10, [rip + lo_bindings]              # t1 = load [@lo_bindings + 24]:
    mov rax, qword ptr [r10 + 24]             #   fetch `out`
    mov r12, rax                              #   t1 lives in r12
    mov qword ptr [rbp - 48], r12             # root_store root0, t1: straight from the register
    lea rdi, [rip + lo_bytes_3]               # t2 = call lo_string_new: arg 1
    mov esi, 13                               #   arg 2 = 13
    call lo_string_new                        #   call
    mov r13, rax                              #   t2 lives in r13
    mov rax, qword ptr [rbp - 48]             # root_load root0:
    mov r12, rax                              #   reload t1 into r12
    mov rax, r12                              # t3 = (t1 == null)
    cmp rax, 0                                #   compare
    sete al                                   #   al = (t1 == 0)
    movzx eax, al                             #   widen to Bool
    mov r14d, eax                             #   t3 lives in r14
    cmp r14d, 0                               # cbr t3
    je .Llo_method_4_Main_4_main_2            #   false -> .L2
.Llo_method_4_Main_4_main_1:                  # IR block .L1 (abort)
    lea rdi, [rip + lo_bytes_4]
    mov esi, 12
    call lo_abort_null_receiver               # call; never returns
.Llo_method_4_Main_4_main_2:                  # IR block .L2
    mov qword ptr [rbp - 48], 0               # root_store root0, null
    mov rdi, r12                              # print_string(t1, t2): receiver straight from r12
    mov rsi, r13                              #   the string straight from r13
    call lo_method_6_Output_12_print_string   #   call
    lea r10, [rip + lo_bindings]              # t4 = load `out` again:
    mov rax, qword ptr [r10 + 24]             #   ...
    mov r15, rax                              #   t4 lives in r15
    mov rax, r15                              # t5 = (t4 == null); `this` is dead, so t5 reuses rbx
    cmp rax, 0
    sete al
    movzx eax, al
    mov ebx, eax
    cmp ebx, 0                                #   ...
    je .Llo_method_4_Main_4_main_4            # cbr t5
.Llo_method_4_Main_4_main_3:                  # IR block .L3 (abort)
    lea rdi, [rip + lo_bytes_5]
    mov esi, 7
    call lo_abort_null_receiver
.Llo_method_4_Main_4_main_4:                  # IR block .L4
    mov qword ptr [rbp - 48], 0               # root_store root0, null
    mov rdi, r15                              # println(t4): receiver straight from r15
    call lo_method_6_Output_7_println         #   call
    mov eax, 0                                # ret 0: eax = 0
    mov qword ptr [rbp - 72], rax             # park the return value
    call lo_pop_frame                         # unlink this function's shadow frame
    mov rax, qword ptr [rbp - 72]             # restore the return value
    mov rbx, qword ptr [rbp - 8]              # restore the five callee-saved registers
    mov r12, qword ptr [rbp - 16]
    mov r13, qword ptr [rbp - 24]
    mov r14, qword ptr [rbp - 32]
    mov r15, qword ptr [rbp - 40]
    leave                                     # mov rsp, rbp ; pop rbp
    ret                                       # return
```

## 4. What changed

| | spill-everything | linear scan |
|---|---|---|
| lines in `Main.main` | 62 | 71 |
| stack references (`[rbp - ...]`) | 25 | 20 |
| `this` | `[rbp - 32]` | `rbx` |
| `t1` (the `out` object) | `[rbp - 40]` | `r12` |
| `t2` (the String) | `[rbp - 48]` | `r13` |
| `t3` (first null check) | `[rbp - 56]` | `r14` |
| `t4` (`out` again) | `[rbp - 64]` | `r15` |
| `t5` (second null check) | `[rbp - 72]` | `rbx` (reused: `this` is dead by then) |
| prologue / epilogue | frame setup only | also saves and restores `rbx, r12–r15` (10 extra instructions) |

For a function this small linear scan is *longer*: the five saves and five restores cost more than the stack traffic it removes. It wins when many values are live. The fifteen-live-ints example, for instance, drops from 161 stack references to 60. The call arguments are the clearest gain here: `mov rdi, r12 ; mov rsi, r13` replaces two loads from the stack.

Everything else (labels, branches, the shadow-frame bookkeeping around each call, the null-check structure) is identical, because only the *location* of each value changed.

## 5. The startup frame order

`lo_entry` is the one function whose shadow frame is linked *after* its first instruction. It builds and nulls the frame in the prologue like any other function, but the runtime resets its frame chain inside `lo_runtime_init`, so linking before that call would be undone:

```
lo_entry:
    push rbp                                  # save the caller's frame pointer
    mov rbp, rsp
    sub rsp, 96                               # reserve 96 bytes
    mov qword ptr [rbp - 8], rbx              # save the callee-saved registers lo_entry uses
    mov qword ptr [rbp - 16], r12
    mov qword ptr [rbp - 24], r13
    mov qword ptr [rbp - 32], r14
    mov qword ptr [rbp - 40], r15
    mov qword ptr [rbp - 88], 0               # shadow frame: parent = 0
    mov qword ptr [rbp - 80], 4               # shadow frame: num_roots = 4
    mov qword ptr [rbp - 72], 0               # shadow frame: roots = null
    mov qword ptr [rbp - 64], 0
    mov qword ptr [rbp - 56], 0
    mov qword ptr [rbp - 48], 0
.Llo_entry_0:                                 # IR block .L0
    mov qword ptr [rbp - 72], 0               # root slots nulled (from the GC-root pass)
    mov qword ptr [rbp - 64], 0
    mov qword ptr [rbp - 56], 0
    mov qword ptr [rbp - 48], 0
    call lo_runtime_init                      # call lo_runtime_init, the first IR instruction. The runtime resets its
    lea rdi, [rbp - 88]                       # frame chain in there, so lo_entry's own frame is linked only now:
    call lo_push_frame                        # call lo_push_frame (the bindings frame is pushed next, by IR)
```
