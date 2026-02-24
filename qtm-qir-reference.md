# Quantinuum QIR Reference

Quantinuum Helios and beyond support [QIR](https://www.qir-alliance.org/)
[Adaptive Profile](https://github.com/qir-alliance/qir-spec/blob/1.0/specification/profiles/Adaptive_Profile.md)
and [Labeled Output Schema](https://github.com/qir-alliance/qir-spec/blob/1.0/specification/output_schemas/Labeled.md).

We document Quantinuum-specific QIS, runtime and platform functions here.

## Quantum Instruction Set

### Native Gate Set

```llvm
declare void @__quantum__qis__rxy__body(double, double, %Qubit*)
declare void @__quantum__qis__rz__body(double, %Qubit*)
declare void @__quantum__qis__rzz__body(double, %Qubit*, %Qubit*)

declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly)
declare void @__quantum__qis__reset__body(%Qubit*)
```

### QIR de facto Gate Set

These gates are decomposed into native gates during compilation.

```llvm
declare void @__quantum__qis__ccx__body(%Qubit*, %Qubit*, %Qubit*)
declare void @__quantum__qis__cx__body(%Qubit*, %Qubit*)
declare void @__quantum__qis__cz__body(%Qubit*, %Qubit*)
declare void @__quantum__qis__h__body(%Qubit*)
declare void @__quantum__qis__rx__body(double, %Qubit*)
declare void @__quantum__qis__ry__body(double, %Qubit*)
declare void @__quantum__qis__s__body(%Qubit*)
declare void @__quantum__qis__s__adj(%Qubit*)
declare void @__quantum__qis__t__body(%Qubit*)
declare void @__quantum__qis__t__adj(%Qubit*)
declare void @__quantum__qis__x__body(%Qubit*)
declare void @__quantum__qis__y__body(%Qubit*)
declare void @__quantum__qis__z__body(%Qubit*)
declare void @__quantum__qis__mresetz__body(%Qubit*, %Result* writeonly)

; Synonym for __quantum__qis__mz__body
declare void @__quantum__qis__m__body(%Qubit*, %Result* writeonly)
; Synonym for __quantum__qis__cx__body
declare void @__quantum__qis__cnot__body(%Qubit*, %Qubit*)
```

#### Decompositions

We show decompositions to the native gate set in a concise syntax.

| QIR (prefixed by `__quantum__qis__`)   | Decomposition to QIR native gates  |
|----------------------------------------|------------------------------------|
| `ccx__body(%Qubit*, %Qubit*, %Qubit*)` | See [below](#toffoli-gate-ccx)     |
| `cx__body(%Qubit*, %Qubit*)`           | See [below](#controlled-x-gate-cx) |
| `cz__body(%Qubit*, %Qubit*)`           | See [below](#controlled-z-gate-cz) |
| `h__body(%Qubit* %q)`                  | `rxy(π/2, -π/2, %q); rz(π, %q)`    |
| `rx__body(double %theta, %Qubit* %q)`  | `rxy(%theta, 0, %q)`               |
| `ry__body(double %theta, %Qubit* %q)`  | `rxy(%theta, π/2, %q)`             |
| `s__body(%Qubit* %q)`                  | `rz(π/2, %q)`                      |
| `s__adj(%Qubit* %q)`                   | `rz(-π/2, %q)`                     |
| `t__body(%Qubit* %q)`                  | `rz(π/4, %q)`                      |
| `t__adj(%Qubit* %q)`                   | `rz(-π/4, %q)`                     |
| `x__body(%Qubit* %q)`                  | `rxy(π, 0, %q)`                    |
| `y__body(%Qubit* %q)`                  | `rxy(π, π/2, %q)`                  |
| `z__body(%Qubit* %q)`                  | `rz(π, %q)`                        |
| `mresetz__body(%Qubit* %q, %result)`   | `mz(%q, %result); reset(%q)`       |

##### Controlled Z gate (CZ)

`__quantum__qis__cz__body(%Qubit* %control, %Qubit* %target)` is decomposed to

```llvm
rzz(π/2, %control, %target);
rz(-π/2, %target);
rz(-π/2, %control);
```

##### Controlled X gate (CX)

`__quantum__qis__cx__body(%Qubit* %control, %Qubit* %target)` is decomposed to

```llvm
rxy(-π/2, π/2, %target);
rzz(π/2, %control, %target);
rz(-π/2, %control);
rxy(π/2, π, %target);
rz(-π/2, %target);
```

##### Toffoli gate (CCX)

`__quantum__qis__ccx__body(%Qubit* %control1, %Qubit* %control2, %Qubit* %target)` is decomposed to

```llvm
rxy(π, -π/2, %target);
rzz(π/2, %control2, %target);
rxy(π/4, π/2, %target);
rzz(π/2, %control1, %target);
rxy(π/4, 0, %target);
rzz(π/2, %control2, %target);
rxy(π/4, -π/2, %target);
rzz(π/2, %control1, %target);
rxy(π, π/4, %control1);
rxy(-3π/4, π, %target);
rzz(π/4, %control1, %control2);
rz(π, %target);
rxy(π, -π/4, %control1);
rz(-3π/4, %control2);
rz(π/4, %control1);
```

### Barrier Instructions

Fixed-arity barrier intrinsics for synchronization:

```llvm
declare void @__quantum__qis__barrier1__body(%Qubit*)
declare void @__quantum__qis__barrier2__body(%Qubit*, %Qubit*)
declare void @__quantum__qis__barrier3__body(%Qubit*, %Qubit*, %Qubit*)
; ... up to an implementation-defined maximum arity
```

where:

`@__quantum__qis__barrier<n>__body(...)` is a barrier over exactly the `n` qubits passed as arguments.

## Runtime Functions

See QIR [Adaptive Profile: §Runtime Functions](https://github.com/qir-alliance/qir-spec/blob/1.0/specification/profiles/Adaptive_Profile.md#runtime-functions)
and [Labeled Output Schema: §Output Recording Functions](https://github.com/qir-alliance/qir-spec/blob/1.0/specification/output_schemas/Labeled.md#output-recording-functions)
for more details.

```llvm
declare void @__quantum__rt__initialize(i8*)
declare i1 @__quantum__rt__read_result(%Result* readonly)

; Output Recording Functions
declare void @__quantum__rt__tuple_record_output(i64, i8*)
declare void @__quantum__rt__array_record_output(i64, i8*)
declare void @__quantum__rt__result_record_output(%Result*, i8*)
declare void @__quantum__rt__bool_record_output(i1, i8*)
declare void @__quantum__rt__int_record_output(i64, i8*)
declare void @__quantum__rt__double_record_output(double, i8*)
```

## Platform Utilities

These QIR functions provide additional runtime capabilities.

### Random Number Generation

```llvm
; Create a new random number generator using a `seed`.
declare void @___random_seed(i64 %seed)
; Generate a random 32-bit signed integer.
declare i32 @___random_int()
; Generate a random floating point value in the range [0,1).
declare double @___random_float()
; Generate a random 32-bit integer in the range [0, `bound`).
declare i32 @___random_int_bounded(i32 %bound)
; Advance or backtrack the RNG state by `delta` steps
declare void @___random_advance(i64 %delta)
```

### Current Shot

```llvm
declare i64 @___get_current_shot()
```
