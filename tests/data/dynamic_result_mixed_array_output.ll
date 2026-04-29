@0 = internal constant [5 x i8] c"mix0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %results = alloca [2 x ptr], align 8
  %r0 = call ptr @__quantum__rt__result_allocate(ptr null)
  %r1 = call ptr @__quantum__rt__result_allocate(ptr null)
  %tmp = insertvalue [2 x ptr] poison, ptr %r0, 0
  %packed = insertvalue [2 x ptr] %tmp, ptr %r1, 1
  store [2 x ptr] %packed, ptr %results, align 8
  %loaded = load [2 x ptr], ptr %results, align 8
  %s0 = extractvalue [2 x ptr] %loaded, 0
  %s1 = extractvalue [2 x ptr] %loaded, 1
  call void @__quantum__qis__mz__body(ptr null, ptr %s0)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %s1)
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %results, ptr @0)
  call void @__quantum__rt__result_array_release(i64 2, ptr %results)
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_array_release(i64, ptr)
declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 true}
