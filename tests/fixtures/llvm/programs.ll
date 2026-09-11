; ModuleID = 'heat.d209bc380f928ec8-cgu.0'
source_filename = "heat.d209bc380f928ec8-cgu.0"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

; Function Attrs: nofree norecurse nosync nounwind nonlazybind memory(none) uwtable
define noundef double @cost(double noundef %x, i64 noundef %n) unnamed_addr #0 {
start:
  %_44 = icmp sgt i64 %n, 0
  br i1 %_44, label %bb2.preheader, label %bb3

bb2.preheader:                                    ; preds = %start
  %xtraiter = and i64 %n, 3
  %0 = icmp ult i64 %n, 4
  br i1 %0, label %bb2.epil.preheader, label %bb2.preheader.new

bb2.preheader.new:                                ; preds = %bb2.preheader
  %unroll_iter = and i64 %n, 9223372036854775804
  br label %bb2

bb3.loopexit.unr-lcssa:                           ; preds = %bb2
  %lcmp.mod.not = icmp eq i64 %xtraiter, 0
  br i1 %lcmp.mod.not, label %bb3, label %bb2.epil.preheader

bb2.epil.preheader:                               ; preds = %bb3.loopexit.unr-lcssa, %bb2.preheader
  %total.sroa.0.06.epil.init = phi double [ 0.000000e+00, %bb2.preheader ], [ %9, %bb3.loopexit.unr-lcssa ]
  %i.sroa.0.05.epil.init = phi i64 [ 0, %bb2.preheader ], [ %10, %bb3.loopexit.unr-lcssa ]
  %lcmp.mod8 = icmp ne i64 %xtraiter, 0
  tail call void @llvm.assume(i1 %lcmp.mod8)
  br label %bb2.epil

bb2.epil:                                         ; preds = %bb2.epil, %bb2.epil.preheader
  %total.sroa.0.06.epil = phi double [ %1, %bb2.epil ], [ %total.sroa.0.06.epil.init, %bb2.epil.preheader ]
  %i.sroa.0.05.epil = phi i64 [ %2, %bb2.epil ], [ %i.sroa.0.05.epil.init, %bb2.epil.preheader ]
  %epil.iter = phi i64 [ %epil.iter.next, %bb2.epil ], [ 0, %bb2.epil.preheader ]
  %_8.epil = uitofp nneg i64 %i.sroa.0.05.epil to double
  %_7.epil = fmul double %x, %_8.epil
  %_6.epil = tail call double @llvm.sin.f64(double %_7.epil)
  %1 = fadd double %total.sroa.0.06.epil, %_6.epil
  %2 = add nuw nsw i64 %i.sroa.0.05.epil, 1
  %epil.iter.next = add i64 %epil.iter, 1
  %epil.iter.cmp.not = icmp eq i64 %epil.iter.next, %xtraiter
  br i1 %epil.iter.cmp.not, label %bb3, label %bb2.epil, !llvm.loop !4

bb3:                                              ; preds = %bb3.loopexit.unr-lcssa, %bb2.epil, %start
  %total.sroa.0.0.lcssa = phi double [ 0.000000e+00, %start ], [ %9, %bb3.loopexit.unr-lcssa ], [ %1, %bb2.epil ]
  ret double %total.sroa.0.0.lcssa

bb2:                                              ; preds = %bb2, %bb2.preheader.new
  %total.sroa.0.06 = phi double [ 0.000000e+00, %bb2.preheader.new ], [ %9, %bb2 ]
  %i.sroa.0.05 = phi i64 [ 0, %bb2.preheader.new ], [ %10, %bb2 ]
  %niter = phi i64 [ 0, %bb2.preheader.new ], [ %niter.next.3, %bb2 ]
  %_8 = uitofp nneg i64 %i.sroa.0.05 to double
  %_7 = fmul double %x, %_8
  %_6 = tail call double @llvm.sin.f64(double %_7)
  %3 = fadd double %total.sroa.0.06, %_6
  %4 = or disjoint i64 %i.sroa.0.05, 1
  %_8.1 = uitofp nneg i64 %4 to double
  %_7.1 = fmul double %x, %_8.1
  %_6.1 = tail call double @llvm.sin.f64(double %_7.1)
  %5 = fadd double %3, %_6.1
  %6 = or disjoint i64 %i.sroa.0.05, 2
  %_8.2 = uitofp nneg i64 %6 to double
  %_7.2 = fmul double %x, %_8.2
  %_6.2 = tail call double @llvm.sin.f64(double %_7.2)
  %7 = fadd double %5, %_6.2
  %8 = or disjoint i64 %i.sroa.0.05, 3
  %_8.3 = uitofp nneg i64 %8 to double
  %_7.3 = fmul double %x, %_8.3
  %_6.3 = tail call double @llvm.sin.f64(double %_7.3)
  %9 = fadd double %7, %_6.3
  %10 = add nuw nsw i64 %i.sroa.0.05, 4
  %niter.next.3 = add i64 %niter, 4
  %niter.ncmp.3 = icmp eq i64 %niter.next.3, %unroll_iter
  br i1 %niter.ncmp.3, label %bb3.loopexit.unr-lcssa, label %bb2
}

; Function Attrs: mustprogress nofree norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable
define noundef double @heat(double noundef %x, double noundef %y) unnamed_addr #1 {
start:
  %_4 = fcmp ogt double %y, 1.000000e+00
  %0 = fmul double %y, 8.000000e-01
  %adjusted.sroa.0.0 = select i1 %_4, double %0, double %y
  %_5 = tail call double @llvm.exp.f64(double %adjusted.sroa.0.0)
  %_0 = fmul double %x, %_5
  ret double %_0
}

; Function Attrs: mustprogress nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare double @llvm.sin.f64(double) #2

; Function Attrs: mustprogress nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none)
declare double @llvm.exp.f64(double) #2

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(inaccessiblemem: write)
declare void @llvm.assume(i1 noundef) #3

attributes #0 = { nofree norecurse nosync nounwind nonlazybind memory(none) uwtable "probe-stack"="inline-asm" "target-cpu"="x86-64" }
attributes #1 = { mustprogress nofree norecurse nosync nounwind nonlazybind willreturn memory(none) uwtable "probe-stack"="inline-asm" "target-cpu"="x86-64" }
attributes #2 = { mustprogress nocallback nocreateundeforpoison nofree nosync nounwind speculatable willreturn memory(none) }
attributes #3 = { nocallback nofree nosync nounwind willreturn memory(inaccessiblemem: write) }

!llvm.module.flags = !{!0, !1, !2}
!llvm.ident = !{!3}

!0 = !{i32 8, !"PIC Level", i32 2}
!1 = !{i32 2, !"RtLibUseGOT", i32 1}
!2 = !{i32 7, !"uwtable", i32 2}
!3 = !{!"rustc version 1.98.0 (88d9e12ae 2026-08-18)"}
!4 = distinct !{!4, !5}
!5 = !{!"llvm.loop.unroll.disable"}
