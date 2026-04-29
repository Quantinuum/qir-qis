@0 = internal constant [8 x i8] c"acheck0\00"
@1 = internal constant [8 x i8] c"acheck1\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %qubits = alloca [2 x ptr], align 8
  %err = alloca i1, align 1
  store i1 false, ptr %err, align 1
  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %qubits, ptr %err)
  %failed = load i1, ptr %err, align 1
  br i1 %failed, label %alloc_failed, label %alloc_ok

alloc_ok:
  %q0_ptr = getelementptr inbounds [2 x ptr], ptr %qubits, i64 0, i64 0
  %q0 = load ptr, ptr %q0_ptr, align 8
  %q1_ptr = getelementptr inbounds [2 x ptr], ptr %qubits, i64 0, i64 1
  %q1 = load ptr, ptr %q1_ptr, align 8
  call void @__quantum__qis__h__body(ptr %q0)
  call void @__quantum__qis__cnot__body(ptr %q0, ptr %q1)
  call void @__quantum__qis__mz__body(ptr %q0, ptr null)
  call void @__quantum__qis__mz__body(ptr %q1, ptr inttoptr (i64 1 to ptr))
  call void @__quantum__rt__result_record_output(ptr null, ptr @0)
  call void @__quantum__rt__result_record_output(ptr inttoptr (i64 1 to ptr), ptr @1)
  call void @__quantum__rt__qubit_array_release(i64 2, ptr %qubits)
  br label %done

alloc_failed:
  br label %done

done:
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
