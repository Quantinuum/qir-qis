@0 = internal constant [5 x i8] c"life\00"

define void @allocate_and_measure(ptr %out, ptr %q) {
entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__qis__mz__body(ptr %q, ptr %r)
  store ptr %r, ptr %out
  ret void
}

define i64 @Entry_Point_Name() #0 {
entry:
  %results = alloca [2 x ptr]
  %second = getelementptr [2 x ptr], ptr %results, i64 0, i64 1
  call void @__quantum__qis__x__body(ptr null)
  br label %loop
loop:
  %i = phi i64 [0, %entry], [%next, %loop]
  call void @allocate_and_measure(ptr %results, ptr null)
  call void @allocate_and_measure(ptr %second, ptr inttoptr (i64 1 to ptr))
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %results, ptr @0)
  call void @__quantum__rt__result_array_release(i64 2, ptr %results)
  %next = add i64 %i, 1
  %more = icmp ult i64 %next, 3
  br i1 %more, label %loop, label %done
done:
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_array_release(i64, ptr)
declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)
declare void @__quantum__qis__x__body(ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 1}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 true}
!5 = !{i32 1, !"ir_functions", i1 true}
!6 = !{i32 1, !"backwards_branching", i1 true}
