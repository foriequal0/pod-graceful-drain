package core

import (
	"context"
	"github.com/go-logr/logr"
	"sync"
	"sync/atomic"
	"time"
)

type Delayer interface {
	NewTask(duration time.Duration, task func(context.Context, bool) error) DelayedTask
	Stop(drain time.Duration, cleanup time.Duration)
}

type delayer struct {
	logger  logr.Logger
	counter int64

	tasksWaitGroup *sync.WaitGroup
	interrupt      chan struct{}
	cleanup        chan struct{}
}

var _ Delayer = &delayer{}

func NewDelayer(logger logr.Logger) Delayer {
	return &delayer{
		logger: logger.WithName("delayer"),

		tasksWaitGroup: &sync.WaitGroup{},
		interrupt:      make(chan struct{}),
		cleanup:        make(chan struct{}),
	}
}

func (d *delayer) NewTask(duration time.Duration, task func(context.Context, bool) error) DelayedTask {
	id := atomic.AddInt64(&d.counter, 1)

	return &delayedTask{
		delayer:  d,
		logger:   d.logger.WithValues("taskId", id),
		id:       DelayedTaskId(id),
		duration: duration,
		task:     task,
	}
}

func (d *delayer) Stop(drain time.Duration, cleanup time.Duration) {
	d.logger.Info("Stopping delayer")

	stopped := make(chan struct{})
	go func() {
		d.tasksWaitGroup.Wait()
		close(stopped)
	}()

	select {
	case <-stopped:
		d.logger.Info("Drained all delayed tasks")
	case <-time.After(drain):
		d.logger.Info("Some delayed tasks are not finished in time. Interrupt and wait them to cleanup")
		close(d.interrupt)

		select {
		case <-stopped:
		case <-time.After(cleanup):
		}
	}
	close(d.cleanup)
	d.logger.Info("Stopped delayer")
}

type DelayedTaskId int64

type DelayedTask interface {
	GetId() DelayedTaskId
	GetDuration() time.Duration
	RunWait(ctx context.Context) error
	RunAsync()
}

type delayedTask struct {
	delayer  *delayer
	logger   logr.Logger
	id       DelayedTaskId
	duration time.Duration
	task     func(context.Context, bool) error
}

var _ DelayedTask = &delayedTask{}

func (t *delayedTask) GetId() DelayedTaskId {
	return t.id
}

func (t *delayedTask) GetDuration() time.Duration {
	return t.duration
}

func (t *delayedTask) RunWait(ctx context.Context) error {
	t.delayer.tasksWaitGroup.Add(1)
	defer t.delayer.tasksWaitGroup.Done()

	innerCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	go func() {
		select {
		case <-innerCtx.Done():
		case <-t.delayer.cleanup:
			cancel()
		}
	}()

	return t.run(innerCtx, t.duration)
}

func (t *delayedTask) RunAsync() {
	t.delayer.tasksWaitGroup.Add(1)
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		select {
		case <-ctx.Done():
		case <-t.delayer.cleanup:
			cancel()
		}
	}()

	go func() {
		defer t.delayer.tasksWaitGroup.Done()
		defer cancel()

		err := t.run(ctx, t.duration)
		_ = err
	}()

	t.logger.V(1).Info("Scheduled delayed task")
}

func (t *delayedTask) run(ctx context.Context, duration time.Duration) error {
	t.logger.Info("Wait timer for", "duration", duration)

	var interrupted bool
	select {
	case <-ctx.Done():
		interrupted = true
	case <-t.delayer.interrupt:
		interrupted = true
	case <-time.After(duration):
		interrupted = false
	}

	t.logger.V(1).Info("Start delayed task", "interrupted", interrupted)

	if t.task != nil {
		newCtx := logr.NewContext(ctx, t.logger)

		if err := t.task(newCtx, interrupted); err != nil {
			t.logger.Error(err, "Delayed task errored")
			return err
		}
	}
	return nil
}
