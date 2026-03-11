; ModuleID = 'qir2_adaptive'
source_filename = "qir2_adaptive.ll"

@0 = internal constant [3 x i8] c"r0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  call void @__quantum__rt__initialize(ptr null)
  call void @__quantum__qis__h__body(ptr null)
  call void @__quantum__qis__mz__body(ptr null, ptr writeonly null)
  %read = call i1 @__quantum__rt__read_result(ptr readonly null)
  br i1 %read, label %then, label %cont

then:
  call void @__quantum__qis__z__body(ptr null)
  br label %cont

cont:
  call void @__quantum__rt__result_record_output(ptr null, ptr @0)
  ret i64 0
}

declare void @__quantum__qis__h__body(ptr)
declare void @__quantum__qis__z__body(ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1
declare i1 @__quantum__rt__read_result(ptr readonly)
declare void @__quantum__rt__initialize(ptr)
declare void @__quantum__rt__result_record_output(ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
