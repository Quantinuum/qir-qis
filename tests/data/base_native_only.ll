; ModuleID = 'qir_base_profile_native_only'
source_filename = "qir_base_profile_native_only.ll"

declare void @__quantum__qis__rxy__body(double, double, %Qubit*)
declare void @__quantum__qis__rz__body(double, %Qubit*)
declare void @__quantum__qis__rzz__body(double, %Qubit*, %Qubit*)

%Qubit = type opaque
%Result = type opaque

@0 = internal constant [3 x i8] c"q0\00"
@1 = internal constant [3 x i8] c"q1\00"
@2 = internal constant [3 x i8] c"t0\00"

define void @entry() #0 {
entry:
    ; Allocate qubits
    call void @__quantum__rt__initialize(i8* null)

    ; Apply RZZ gate to q0 and q1
    call void @__quantum__qis__rzz__body(double 2.3562, %Qubit* null, %Qubit* inttoptr (i64 1 to %Qubit*))

    ; Apply RXY gate to q0
    call void @__quantum__qis__rxy__body(double 1.5708, double 0.7854, %Qubit* null)

    ; Apply RZ gate to q1
    call void @__quantum__qis__rz__body(double 3.1415, %Qubit* inttoptr (i64 1 to %Qubit*))

    ; Measure qubits
    call void @__quantum__qis__mz__body(%Qubit* null, %Result* writeonly null)
    call void @__quantum__qis__mz__body(%Qubit* inttoptr (i64 1 to %Qubit*), %Result* writeonly inttoptr (i64 1 to %Result*))


    call void @__quantum__rt__tuple_record_output(i64 2, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @2, i32 0, i32 0))
    call void @__quantum__rt__result_record_output(%Result* null, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @0, i32 0, i32 0))
    call void @__quantum__rt__result_record_output(%Result* inttoptr (i64 1 to %Result*), i8* getelementptr inbounds ([3 x i8], [3 x i8]* @1, i32 0, i32 0))
    ret void
}

declare void @__quantum__rt__initialize(i8*)
declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly) #1
declare void @__quantum__rt__tuple_record_output(i64, i8*)
declare void @__quantum__rt__result_record_output(%Result*, i8*)

attributes #0 = { "entry_point" "output_labeling_schema"="schema_id" "qir_profiles"="custom" "required_num_qubits"="2" "required_num_results"="2" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
