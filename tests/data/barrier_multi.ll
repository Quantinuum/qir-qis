; QIR test for barrier instructions with larger arity (multi-digit)

%Result = type opaque
%Qubit = type opaque

; global constants (labels for output recording)

@0 = internal constant [3 x i8] c"r1\00"
@1 = internal constant [3 x i8] c"r2\00"
@2 = internal constant [3 x i8] c"r3\00"
@3 = internal constant [3 x i8] c"r4\00"
@4 = internal constant [3 x i8] c"r5\00"
@5 = internal constant [3 x i8] c"t0\00"

; entry point definition

define i64 @test_barriers_multi() #0 {
entry:
  ; calls to initialize the execution environment
  call void @__quantum__rt__initialize(i8* null)
  br label %body

body:                                     ; preds = %entry
  ; Apply some gates to multiple qubits
  call void @__quantum__qis__h__body(%Qubit* null)
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 1 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 2 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 3 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 4 to %Qubit*))

  ; Barrier on 5 qubits
  call void @__quantum__qis__barrier5__body(%Qubit* null, %Qubit* inttoptr (i64 1 to %Qubit*), %Qubit* inttoptr (i64 2 to %Qubit*), %Qubit* inttoptr (i64 3 to %Qubit*), %Qubit* inttoptr (i64 4 to %Qubit*))

  ; Apply more gates
  call void @__quantum__qis__cnot__body(%Qubit* null, %Qubit* inttoptr (i64 1 to %Qubit*))

  ; Test multi-digit barrier arity: barrier on 12 qubits
  call void @__quantum__qis__barrier12__body(%Qubit* null, %Qubit* inttoptr (i64 1 to %Qubit*), %Qubit* inttoptr (i64 2 to %Qubit*), %Qubit* inttoptr (i64 3 to %Qubit*), %Qubit* inttoptr (i64 4 to %Qubit*), %Qubit* inttoptr (i64 5 to %Qubit*), %Qubit* inttoptr (i64 6 to %Qubit*), %Qubit* inttoptr (i64 7 to %Qubit*), %Qubit* inttoptr (i64 8 to %Qubit*), %Qubit* inttoptr (i64 9 to %Qubit*), %Qubit* inttoptr (i64 10 to %Qubit*), %Qubit* inttoptr (i64 11 to %Qubit*))

  br label %measurements

measurements:                             ; preds = %body
  ; calls to QIS functions that are irreversible
  call void @__quantum__qis__mz__body(%Qubit* null, %Result* writeonly null)
  call void @__quantum__qis__mz__body(%Qubit* inttoptr (i64 1 to %Qubit*), %Result* writeonly inttoptr (i64 1 to %Result*))
  call void @__quantum__qis__mz__body(%Qubit* inttoptr (i64 2 to %Qubit*), %Result* writeonly inttoptr (i64 2 to %Result*))
  call void @__quantum__qis__mz__body(%Qubit* inttoptr (i64 3 to %Qubit*), %Result* writeonly inttoptr (i64 3 to %Result*))
  call void @__quantum__qis__mz__body(%Qubit* inttoptr (i64 4 to %Qubit*), %Result* writeonly inttoptr (i64 4 to %Result*))
  br label %output

output:                                   ; preds = %measurements
  ; calls to record the program output
  call void @__quantum__rt__tuple_record_output(i64 5, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @5, i32 0, i32 0))
  call void @__quantum__rt__result_record_output(%Result* null, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @0, i32 0, i32 0))
  call void @__quantum__rt__result_record_output(%Result* inttoptr (i64 1 to %Result*), i8* getelementptr inbounds ([3 x i8], [3 x i8]* @1, i32 0, i32 0))
  call void @__quantum__rt__result_record_output(%Result* inttoptr (i64 2 to %Result*), i8* getelementptr inbounds ([3 x i8], [3 x i8]* @2, i32 0, i32 0))
  call void @__quantum__rt__result_record_output(%Result* inttoptr (i64 3 to %Result*), i8* getelementptr inbounds ([3 x i8], [3 x i8]* @3, i32 0, i32 0))
  call void @__quantum__rt__result_record_output(%Result* inttoptr (i64 4 to %Result*), i8* getelementptr inbounds ([3 x i8], [3 x i8]* @4, i32 0, i32 0))

  ret i64 0
}

; QIS function declarations

declare void @__quantum__rt__initialize(i8*)

declare void @__quantum__qis__h__body(%Qubit*)

declare void @__quantum__qis__x__body(%Qubit*)

declare void @__quantum__qis__cnot__body(%Qubit*, %Qubit*)

declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly) #1

declare void @__quantum__qis__barrier5__body(%Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*)

declare void @__quantum__qis__barrier12__body(%Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*, %Qubit*)

declare void @__quantum__rt__tuple_record_output(i64, i8*)

declare void @__quantum__rt__result_record_output(%Result*, i8*)

attributes #0 = { "entry_point" "output_labeling_schema" "qir_profiles"="base_profile" "required_num_qubits"="12" "required_num_results"="5" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
