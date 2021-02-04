package core_test

import (
	"context"
	"errors"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"gotest.tools/assert"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	"testing"
	"time"
)

func newDelayer() core.Delayer {
	return core.NewDelayer(zap.New())
}

type taskProbe struct {
	contextErr  error
	interrupted bool
	time        time.Time
}

func newTaskProbe(ctx context.Context, interrupted bool) taskProbe {
	return taskProbe{
		contextErr:  ctx.Err(),
		interrupted: interrupted,
		time:        time.Now(),
	}
}

const (
	shortDuration = 30 * time.Millisecond
	halfDuration  = 50 * time.Millisecond
	duration      = 100 * time.Millisecond
	longDuration  = 200 * time.Millisecond
)

func TestDelayedTask_RunAfterWait_ShouldBlock(t *testing.T) {
	delayer := newDelayer()
	defer delayer.Stop(duration, duration)
	probe := make(chan taskProbe, 1)
	task := delayer.NewTask(func(ctx context.Context, interrupted bool) error {
		probe <- newTaskProbe(ctx, interrupted)
		return nil
	})

	_ = task.RunAfterWait(context.TODO(), duration)

	select {
	case <-probe:
	default:
		assert.Assert(t, false, "Task should've ran when RunAfterWait returned")
	}
}

func TestDelayedTask_RunAfterWait_ShouldCancelledAfterTimeout(t *testing.T) {
	delayer := newDelayer()
	defer delayer.Stop(duration, duration)
	probe := make(chan taskProbe, 1)
	task := delayer.NewTask(func(ctx context.Context, interrupted bool) error {
		probe <- newTaskProbe(ctx, interrupted)
		return nil
	})

	ctx, cancel := context.WithTimeout(context.TODO(), shortDuration)
	defer cancel()
	start := time.Now()
	_ = task.RunAfterWait(ctx, duration)

	select {
	case probeResult := <-probe:
		assert.Equal(t, probeResult.interrupted, true, "Task should be interrupted")
		assert.Equal(t, probeResult.contextErr, context.DeadlineExceeded, "Task should be timed out")
		assert.Equal(t, start.Sub(probeResult.time) < duration, true, "Task should be started earlier")
	default:
		assert.Assert(t, false, "Task should've ran when RunAfterWait returned")
	}
}

func TestDelayedTask_RunAfterWait_ShouldPassError(t *testing.T) {
	tests := []struct {
		name string
		err  error
	}{{"nil", nil}, {"err", errors.New("error")}}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			delayer := newDelayer()
			defer delayer.Stop(duration, duration)
			err1 := errors.New("error")
			task := delayer.NewTask(func(ctx context.Context, _ bool) error {
				return err1
			})

			err2 := task.RunAfterWait(context.TODO(), time.Duration(0))

			assert.Equal(t, err1, err2, "RunAfterWait should return what task have returned")
		})
	}
}

func TestDelayedTask_RunAfterWait_ShouldNotBlock(t *testing.T) {
	delayer := newDelayer()
	defer delayer.Stop(duration, duration)
	probe := make(chan taskProbe, 1)
	task := delayer.NewTask(func(ctx context.Context, interrupted bool) error {
		probe <- newTaskProbe(ctx, interrupted)
		<-ctx.Done()
		return nil
	})

	task.RunAfterAsync(duration)

	select {
	case <-probe:
		assert.Assert(t, false, "Task shouldn't have ran when RunAfterAsync returned")
	default:
	}
}

func TestDelayedTask_NoInterruptDrain_WhenDelayIsShortEnough(t *testing.T) {
	givenDelay := duration
	givenDrain := longDuration

	tests := []struct {
		name   string
		runner func(task core.DelayedTask, delay time.Duration)
	}{
		{"RunAfterWait", func(task core.DelayedTask, delay time.Duration) { _ = task.RunAfterWait(context.TODO(), delay) }},
		{"RunAfterAsync", func(task core.DelayedTask, delay time.Duration) { task.RunAfterAsync(delay) }},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			delayer := newDelayer()
			probe := make(chan taskProbe, 1)
			task := delayer.NewTask(func(ctx context.Context, interrupted bool) error {
				probe <- newTaskProbe(ctx, interrupted) // probeResult
				return nil
			})

			stopperChan := make(chan time.Time, 2)
			go func() {
				stopperChan <- time.Now() // stopperStart
				delayer.Stop(givenDrain, duration)
				stopperChan <- time.Now() // stopperEnd
			}()

			start := time.Now()
			tt.runner(task, givenDelay)

			stopperStart := <-stopperChan
			probeResult := <-probe
			stopperEnd := <-stopperChan

			delay := probeResult.time.Sub(start)
			stop := stopperEnd.Sub(stopperStart)

			assert.Equal(t, probeResult.interrupted, false, "Task should not be interrupted")
			assert.Assert(t, delay >= givenDelay, "Task should be delayed enough")
			assert.Assert(t, stop <= givenDrain, "delayer should stop as soon as all tasks are drained")
		})
	}
}

func TestDelayedTask_InterruptedDrain_WhenDelayIsTooLong(t *testing.T) {
	givenDelay := longDuration
	givenDrain := duration
	givenCleanup := halfDuration
	givenStop := givenDrain + givenCleanup

	tests := []struct {
		name   string
		runner func(task core.DelayedTask, delay time.Duration)
	}{
		{"RunAfterWait", func(task core.DelayedTask, delay time.Duration) { _ = task.RunAfterWait(context.Background(), delay) }},
		{"RunAfterAsync", func(task core.DelayedTask, delay time.Duration) { task.RunAfterAsync(delay) }},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			delayer := newDelayer()
			probe := make(chan taskProbe, 2)
			task := delayer.NewTask(func(ctx context.Context, interrupted bool) error {
				probe <- newTaskProbe(ctx, interrupted) // interrupted
				<-ctx.Done()
				probe <- newTaskProbe(ctx, interrupted) // cancelled
				return nil
			})

			stopperChan := make(chan time.Time, 2)
			go func() {
				stopperChan <- time.Now() // stopperStart
				delayer.Stop(givenDrain, givenCleanup)
				stopperChan <- time.Now() // stopperEnd
			}()

			start := time.Now()
			tt.runner(task, givenDelay)

			stopperStart := <-stopperChan
			interrupted := <-probe
			cancelled := <-probe
			stopperEnd := <-stopperChan

			delay := interrupted.time.Sub(start)
			drain := interrupted.time.Sub(stopperStart)
			cleanup := cancelled.time.Sub(interrupted.time)
			stop := stopperEnd.Sub(stopperStart)

			assert.Equal(t, interrupted.interrupted, true, "Task should be interrupted")
			assert.NilError(t, interrupted.contextErr, "Task should not be cancelled yet")
			assert.Equal(t, cancelled.contextErr, context.Canceled, "Task should be cancelled")

			assert.Assert(t, delay < givenDelay, "Task should be started earlier")
			assert.Assert(t, similar(drain, givenDrain, 0.1), "delayer should allow tasks delayed up to drain period")
			assert.Assert(t, similar(cleanup, givenCleanup, 0.1), "delayer should wait task to finish")

			assert.Assert(t, similar(stop, givenStop, 0.1), "delayer should stop in time")
		})
	}
}

func TestDelayedTask_EmptyStop(t *testing.T) {
	givenDelay := shortDuration
	stopSleep := duration

	tests := []struct {
		name   string
		runner func(task core.DelayedTask, delay time.Duration)
	}{
		{"RunAfterWait", func(task core.DelayedTask, delay time.Duration) { _ = task.RunAfterWait(context.Background(), delay) }},
		{"RunAfterAsync", func(task core.DelayedTask, delay time.Duration) { task.RunAfterAsync(delay) }},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			delayer := newDelayer()
			task := delayer.NewTask(nil)

			stopperChan := make(chan time.Time, 2)
			go func() {
				time.Sleep(stopSleep)
				stopperChan <- time.Now() // stopperStart
				delayer.Stop(duration, duration)
				stopperChan <- time.Now() // stopperEnd
			}()

			tt.runner(task, givenDelay)

			stopperStart := <-stopperChan
			stopperEnd := <-stopperChan

			stop := stopperEnd.Sub(stopperStart)

			assert.Assert(t, stop < shortDuration, "delayer should stop immediately")
		})
	}
}
func similar(x time.Duration, y time.Duration, toleration float64) bool {
	xs := x.Seconds()
	ys := y.Seconds()
	return xs >= ys*(1.0-toleration) && xs <= ys*(1.0+toleration)
}
