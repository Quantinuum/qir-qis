%Result = type opaque
%Qubit = type opaque

; printed randomly
@r0 = internal constant [3 x i8] c"r0\00"
@r1 = internal constant [3 x i8] c"r1\00"
@r2 = internal constant [3 x i8] c"r2\00"

; always printed
@s0 = internal constant [3 x i8] c"s0\00"
@i0 = internal constant [3 x i8] c"i0\00"
@i1 = internal constant [3 x i8] c"i1\00"
@f0 = internal constant [3 x i8] c"f0\00"
@rng_int1 = internal constant [9 x i8] c"rng_int1\00"
@rng_int2 = internal constant [9 x i8] c"rng_int2\00"
@rng_flt = internal constant [8 x i8] c"rng_flt\00"
@rng_intb = internal constant [9 x i8] c"rng_intb\00"

define void @ENTRYPOINT__main() #0 {
block_0:
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 0 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 1 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 2 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 3 to %Qubit*))
  call void @__quantum__qis__x__body(%Qubit* inttoptr (i64 4 to %Qubit*))
  call void @__quantum__qis__m__body(%Qubit* inttoptr (i64 0 to %Qubit*), %Result* inttoptr (i64 0 to %Result*))
  call void @__quantum__qis__m__body(%Qubit* inttoptr (i64 1 to %Qubit*), %Result* inttoptr (i64 1 to %Result*))
  call void @__quantum__qis__m__body(%Qubit* inttoptr (i64 2 to %Qubit*), %Result* inttoptr (i64 2 to %Result*))
  call void @__quantum__qis__m__body(%Qubit* inttoptr (i64 3 to %Qubit*), %Result* inttoptr (i64 3 to %Result*))
  call void @__quantum__qis__m__body(%Qubit* inttoptr (i64 4 to %Qubit*), %Result* inttoptr (i64 4 to %Result*))
  %var_8 = call i1 @__quantum__rt__read_result(%Result* inttoptr (i64 0 to %Result*))
  br i1 %var_8, label %block_1, label %block_2
block_1:
  br label %block_2
block_2:
  %var_39 = phi i64 [1, %block_0], [3, %block_1]
  %var_38 = phi i64 [10, %block_0], [8, %block_1]
  %var_37 = phi i64 [0, %block_0], [5, %block_1]
  %var_36 = phi i64 [0, %block_0], [1, %block_1]
  %var_10 = call i1 @__quantum__rt__read_result(%Result* inttoptr (i64 1 to %Result*))
  br i1 %var_10, label %block_3, label %block_4
block_3:
  %var_12 = add i64 %var_36, 1
  %var_13 = add i64 %var_37, 5
  %var_14 = sub i64 %var_38, 2
  %var_15 = mul i64 %var_39, 3
  br label %block_4
block_4:
  %var_43 = phi i64 [%var_39, %block_2], [%var_15, %block_3]
  %var_42 = phi i64 [%var_38, %block_2], [%var_14, %block_3]
  %var_41 = phi i64 [%var_37, %block_2], [%var_13, %block_3]
  %var_40 = phi i64 [%var_36, %block_2], [%var_12, %block_3]
  %var_16 = call i1 @__quantum__rt__read_result(%Result* inttoptr (i64 2 to %Result*))
  br i1 %var_16, label %block_5, label %block_6
block_5:
  %var_18 = add i64 %var_40, 1
  %var_19 = add i64 %var_41, 5
  %var_20 = sub i64 %var_42, 2
  %var_21 = mul i64 %var_43, 3
  br label %block_6
block_6:
  %var_47 = phi i64 [%var_43, %block_4], [%var_21, %block_5]
  %var_46 = phi i64 [%var_42, %block_4], [%var_20, %block_5]
  %var_45 = phi i64 [%var_41, %block_4], [%var_19, %block_5]
  %var_44 = phi i64 [%var_40, %block_4], [%var_18, %block_5]
  %var_22 = call i1 @__quantum__rt__read_result(%Result* inttoptr (i64 3 to %Result*))
  br i1 %var_22, label %block_7, label %block_8
block_7:
  %var_24 = add i64 %var_44, 1
  %var_25 = add i64 %var_45, 5
  %var_26 = sub i64 %var_46, 2
  %var_27 = mul i64 %var_47, 3
  br label %block_8
block_8:
  %var_51 = phi i64 [%var_47, %block_6], [%var_27, %block_7]
  %var_50 = phi i64 [%var_46, %block_6], [%var_26, %block_7]
  %var_49 = phi i64 [%var_45, %block_6], [%var_25, %block_7]
  %var_48 = phi i64 [%var_44, %block_6], [%var_24, %block_7]
  %var_28 = call i1 @__quantum__rt__read_result(%Result* inttoptr (i64 4 to %Result*))
  br i1 %var_28, label %block_9, label %block_10
block_9:
  %var_30 = add i64 %var_48, 1
  %var_31 = add i64 %var_49, 5
  %var_32 = sub i64 %var_50, 2
  %var_33 = mul i64 %var_51, 3
  br label %block_10
block_10:
  %var_55 = phi i64 [%var_51, %block_8], [%var_33, %block_9]
  %var_54 = phi i64 [%var_50, %block_8], [%var_32, %block_9]
  %var_53 = phi i64 [%var_49, %block_8], [%var_31, %block_9]
  %var_52 = phi i64 [%var_48, %block_8], [%var_30, %block_9]
  call void @__quantum__qis__reset__body(%Qubit* inttoptr (i64 0 to %Qubit*))
  call void @__quantum__qis__reset__body(%Qubit* inttoptr (i64 1 to %Qubit*))
  call void @__quantum__qis__reset__body(%Qubit* inttoptr (i64 2 to %Qubit*))
  call void @__quantum__qis__reset__body(%Qubit* inttoptr (i64 3 to %Qubit*))
  call void @__quantum__qis__reset__body(%Qubit* inttoptr (i64 4 to %Qubit*))

  ; get and print the shot number
  %shot = call i64 @___get_current_shot()
  call void @__quantum__rt__int_record_output(i64 %shot, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @s0, i32 0, i32 0))

  call void @___random_seed(i64 42)

  ; Generate a random integer
  %rint  = call i32 @___random_int()
  %rint64 = sext i32 %rint to i64
  call void @__quantum__rt__int_record_output(i64 %rint64, i8* getelementptr inbounds ([9 x i8], [9 x i8]* @rng_int1, i32 0, i32 0))

  ; Advance the random number generator so we can regenerate the same random number
  call void @___random_advance(i64 -1)

  %rinta  = call i32 @___random_int()
  %rint64a = sext i32 %rinta to i64
  call void @__quantum__rt__int_record_output(i64 %rint64a, i8* getelementptr inbounds ([9 x i8], [9 x i8]* @rng_int2, i32 0, i32 0))

  ; Generate a random float
  %rfloat  = call double @___random_float()
  call void @__quantum__rt__double_record_output(double %rfloat, i8* getelementptr inbounds ([8 x i8], [8 x i8]* @rng_flt, i32 0, i32 0))
  br label %random

random:
  ; Generate a random integer between 0 and 4
  %val = call i32 @___random_int_bounded(i32 5)
  %val64 = sext i32 %val to i64
  call void @__quantum__rt__int_record_output(i64 %val64, i8* getelementptr inbounds ([9 x i8], [9 x i8]* @rng_intb, i32 0, i32 0))
  switch i32 %val, label %otherwise [ i32 0, label %onzero
                                      i32 1, label %onone
                                      i32 2, label %ontwo
                                    ]

onzero:
  call void @__quantum__rt__int_record_output(i64 %var_52, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @r0, i32 0, i32 0))
  br label %random
onone:
  call void @__quantum__rt__int_record_output(i64 %var_53, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @r1, i32 0, i32 0))
  br label %random
ontwo:
  call void @__quantum__rt__int_record_output(i64 %var_54, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @r2, i32 0, i32 0))
  br label %random
otherwise:
  call void @__quantum__rt__int_record_output(i64 %var_55, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @i0, i32 0, i32 0))
  call void @__quantum__rt__int_record_output(i64 %rint64, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @i1, i32 0, i32 0))
  call void @__quantum__rt__double_record_output(double %rfloat, i8* getelementptr inbounds ([3 x i8], [3 x i8]* @f0, i32 0, i32 0))
  ret void
}

declare void @__quantum__qis__x__body(%Qubit*)

declare void @__quantum__qis__m__body(%Qubit*, %Result*) #1

declare i1 @__quantum__rt__read_result(%Result*)

declare void @__quantum__qis__reset__body(%Qubit*) #1

declare void @__quantum__rt__int_record_output(i64, i8*)
declare void @__quantum__rt__double_record_output(double, i8*)

declare i64 @___get_current_shot()
declare void @___random_seed(i64)
declare i32 @___random_int()
declare double @___random_float()
declare i32 @___random_int_bounded(i32)
declare void @___random_advance(i64)

attributes #0 = { "entry_point" "output_labeling_schema" "qir_profiles"="adaptive_profile" "required_num_qubits"="5" "required_num_results"="6" }
attributes #1 = { "irreversible" }

; module flags

!llvm.module.flags = !{!0, !1, !2, !3, !4, !5, !6, !7, !8, !9}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 5, !"int_computations", !10}
!5 = !{i32 5, !"float_computations", !11}
!6 = !{i32 1, !"ir_functions", i1 false}
!7 = !{i32 1, !"backwards_branching", i2 0}
!8 = !{i32 1, !"multiple_target_branching", i1 true}
!9 = !{i32 1, !"multiple_return_points", i1 false}
!10 = !{!"i32", !"i64"}
!11 = !{!"float", !"double"}
