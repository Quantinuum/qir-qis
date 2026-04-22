@0 = internal constant [3 x i8] c"r0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__qis__x__body(ptr null)
  call void @__quantum__qis__mz__body(ptr null, ptr %r)
  call void @__quantum__rt__result_record_output(ptr %r, ptr @0)
  call void @__quantum__rt__result_release(ptr %r)
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)
declare void @__quantum__qis__x__body(ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1
declare void @__quantum__rt__result_record_output(ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 false}
