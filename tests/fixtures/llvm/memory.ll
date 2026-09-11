; ModuleID = 'memory.6c65ff2586f01e0d-cgu.0'
source_filename = "memory.6c65ff2586f01e0d-cgu.0"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

; Function Attrs: nofree norecurse nosync nounwind nonlazybind memory(argmem: read) uwtable
define noundef double @dot(ptr noundef readonly captures(none) %a, ptr noundef readonly captures(none) %b, i64 noundef %n) unnamed_addr #0 {
start:
  %_55 = icmp sgt i64 %n, 0
  br i1 %_55, label %bb2.preheader, label %bb3

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
  %s.sroa.0.07.epil.init = phi double [ 0.000000e+00, %bb2.preheader ], [ %9, %bb3.loopexit.unr-lcssa ]
  %i.sroa.0.06.epil.init = phi i64 [ 0, %bb2.preheader ], [ %10, %bb3.loopexit.unr-lcssa ]
  %lcmp.mod9 = icmp ne i64 %xtraiter, 0
  tail call void @llvm.assume(i1 %lcmp.mod9)
  br label %bb2.epil

bb2.epil:                                         ; preds = %bb2.epil, %bb2.epil.preheader
  %s.sroa.0.07.epil = phi double [ %1, %bb2.epil ], [ %s.sroa.0.07.epil.init, %bb2.epil.preheader ]
  %i.sroa.0.06.epil = phi i64 [ %2, %bb2.epil ], [ %i.sroa.0.06.epil.init, %bb2.epil.preheader ]
  %epil.iter = phi i64 [ %epil.iter.next, %bb2.epil ], [ 0, %bb2.epil.preheader ]
  %_9.epil = getelementptr inbounds nuw double, ptr %a, i64 %i.sroa.0.06.epil
  %_8.epil = load double, ptr %_9.epil, align 8, !noundef !4
  %_13.epil = getelementptr inbounds nuw double, ptr %b, i64 %i.sroa.0.06.epil
  %_12.epil = load double, ptr %_13.epil, align 8, !noundef !4
  %_7.epil = fmul double %_8.epil, %_12.epil
  %1 = fadd double %s.sroa.0.07.epil, %_7.epil
  %2 = add nuw nsw i64 %i.sroa.0.06.epil, 1
  %epil.iter.next = add i64 %epil.iter, 1
  %epil.iter.cmp.not = icmp eq i64 %epil.iter.next, %xtraiter
  br i1 %epil.iter.cmp.not, label %bb3, label %bb2.epil, !llvm.loop !5

bb3:                                              ; preds = %bb3.loopexit.unr-lcssa, %bb2.epil, %start
  %s.sroa.0.0.lcssa = phi double [ 0.000000e+00, %start ], [ %9, %bb3.loopexit.unr-lcssa ], [ %1, %bb2.epil ]
  ret double %s.sroa.0.0.lcssa

bb2:                                              ; preds = %bb2, %bb2.preheader.new
  %s.sroa.0.07 = phi double [ 0.000000e+00, %bb2.preheader.new ], [ %9, %bb2 ]
  %i.sroa.0.06 = phi i64 [ 0, %bb2.preheader.new ], [ %10, %bb2 ]
  %niter = phi i64 [ 0, %bb2.preheader.new ], [ %niter.next.3, %bb2 ]
  %_9 = getelementptr inbounds nuw double, ptr %a, i64 %i.sroa.0.06
  %_8 = load double, ptr %_9, align 8, !noundef !4
  %_13 = getelementptr inbounds nuw double, ptr %b, i64 %i.sroa.0.06
  %_12 = load double, ptr %_13, align 8, !noundef !4
  %_7 = fmul double %_8, %_12
  %3 = fadd double %s.sroa.0.07, %_7
  %4 = or disjoint i64 %i.sroa.0.06, 1
  %_9.1 = getelementptr inbounds nuw double, ptr %a, i64 %4
  %_8.1 = load double, ptr %_9.1, align 8, !noundef !4
  %_13.1 = getelementptr inbounds nuw double, ptr %b, i64 %4
  %_12.1 = load double, ptr %_13.1, align 8, !noundef !4
  %_7.1 = fmul double %_8.1, %_12.1
  %5 = fadd double %3, %_7.1
  %6 = or disjoint i64 %i.sroa.0.06, 2
  %_9.2 = getelementptr inbounds nuw double, ptr %a, i64 %6
  %_8.2 = load double, ptr %_9.2, align 8, !noundef !4
  %_13.2 = getelementptr inbounds nuw double, ptr %b, i64 %6
  %_12.2 = load double, ptr %_13.2, align 8, !noundef !4
  %_7.2 = fmul double %_8.2, %_12.2
  %7 = fadd double %5, %_7.2
  %8 = or disjoint i64 %i.sroa.0.06, 3
  %_9.3 = getelementptr inbounds nuw double, ptr %a, i64 %8
  %_8.3 = load double, ptr %_9.3, align 8, !noundef !4
  %_13.3 = getelementptr inbounds nuw double, ptr %b, i64 %8
  %_12.3 = load double, ptr %_13.3, align 8, !noundef !4
  %_7.3 = fmul double %_8.3, %_12.3
  %9 = fadd double %7, %_7.3
  %10 = add nuw nsw i64 %i.sroa.0.06, 4
  %niter.next.3 = add i64 %niter, 4
  %niter.ncmp.3 = icmp eq i64 %niter.next.3, %unroll_iter
  br i1 %niter.ncmp.3, label %bb3.loopexit.unr-lcssa, label %bb2
}

; Function Attrs: nocallback nofree nosync nounwind willreturn memory(inaccessiblemem: write)
declare void @llvm.assume(i1 noundef) #1

attributes #0 = { nofree norecurse nosync nounwind nonlazybind memory(argmem: read) uwtable "probe-stack"="inline-asm" "target-cpu"="x86-64" }
attributes #1 = { nocallback nofree nosync nounwind willreturn memory(inaccessiblemem: write) }

!llvm.module.flags = !{!0, !1, !2}
!llvm.ident = !{!3}

!0 = !{i32 8, !"PIC Level", i32 2}
!1 = !{i32 2, !"RtLibUseGOT", i32 1}
!2 = !{i32 7, !"uwtable", i32 2}
!3 = !{!"rustc version 1.98.0 (88d9e12ae 2026-08-18)"}
!4 = !{}
!5 = distinct !{!5, !6}
!6 = !{!"llvm.loop.unroll.disable"}
