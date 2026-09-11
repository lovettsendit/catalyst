; ModuleID = 'rules_wrong.2282e8840f40f9c6-cgu.0'
source_filename = "rules_wrong.2282e8840f40f9c6-cgu.0"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable
define noundef double @special_lookup(double noundef %x) unnamed_addr #0 {
start:
  %_2 = fmul double %x, %x
  %_0 = fmul double %x, %_2
  ret double %_0
}

; Function Attrs: mustprogress nofree noinline norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable
define noundef double @special_lookup_gradient(double noundef %x) unnamed_addr #0 {
start:
  %_0 = fmul double %x, 2.000000e+00
  ret double %_0
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable
define noundef double @table_cost(double noundef %x, double noundef %y) unnamed_addr #1 {
start:
  %_4 = fmul double %x, %y
  %_3 = tail call noundef double @special_lookup(double noundef %_4) #2
  %_5 = fmul double %y, %y
  %_0 = fadd double %_5, %_3
  ret double %_0
}

attributes #0 = { mustprogress nofree noinline norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable "probe-stack"="inline-asm" "target-cpu"="x86-64" }
attributes #1 = { mustprogress nofree norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable "probe-stack"="inline-asm" "target-cpu"="x86-64" }
attributes #2 = { noinline nounwind }

!llvm.module.flags = !{!0, !1, !2}
!llvm.ident = !{!3}

!0 = !{i32 8, !"PIC Level", i32 2}
!1 = !{i32 2, !"RtLibUseGOT", i32 1}
!2 = !{i32 7, !"uwtable", i32 2}
!3 = !{!"rustc version 1.98.0 (88d9e12ae 2026-08-18)"}
