@0 = internal constant [3 x i8] c"m0\00"
@1 = internal constant [3 x i8] c"m1\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %qubits = alloca [2 x ptr], align 8
  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %qubits, ptr null)
  %loaded = load [2 x ptr], ptr %qubits, align 8
  %q1 = extractvalue [2 x ptr] %loaded, 1
  %tmp = insertvalue [2 x ptr] poison, ptr %q1, 0
  %q0 = extractvalue [2 x ptr] %loaded, 0
  %swapped = insertvalue [2 x ptr] %tmp, ptr %q0, 1
  %first = extractvalue [2 x ptr] %swapped, 0
  %second = extractvalue [2 x ptr] %swapped, 1
  call void @__quantum__qis__h__body(ptr %first)
  call void @__quantum__qis__cnot__body(ptr %first, ptr %second)
  call void @__quantum__qis__mz__body(ptr %first, ptr null)
  call void @__quantum__qis__mz__body(ptr %second, ptr inttoptr (i64 1 to ptr))
  call void @__quantum__rt__result_record_output(ptr null, ptr @0)
  call void @__quantum__rt__result_record_output(ptr inttoptr (i64 1 to ptr), ptr @1)
  call void @__quantum__rt__qubit_array_release(i64 2, ptr %qubits)
  ret i64 0
}

declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)
declare void @__quantum__rt__qubit_array_release(i64, ptr)
declare void @__quantum__qis__h__body(ptr)
declare void @__quantum__qis__cnot__body(ptr, ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1
declare void @__quantum__rt__result_record_output(ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="2" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 true}
