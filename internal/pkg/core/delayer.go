package core

import (
	"context"
	"github.com/go-logr/logr"
	"sync"
	"sync/atomic"
	"time"
)

type Delayer interface {
	NewTask(task func(context.Context, bool) error) DelayedTask
	Stop(drain time.Duration, cleanup time.Duration)
}

var _ Delayer = &delayer{}

type delayer struct {
	logger  logr.Logger
	counter int64

	interrupt      chan struct{}
	tasksWaitGroup *sync.WaitGroup
	cleanupContext context.Context
	cleanupCancel  context.CancelFunc
}

func NewDelayer(logger logr.Logger) Delayer {
	cleanupContext, cleanupCancel := context.WithCancel(context.Background())

	return &delayer{
		logger: logger.WithName("delayer"),

		interrupt:      make(chan struct{}),
		tasksWaitGroup: &sync.WaitGroup{},
		cleanupContext: cleanupContext,
		cleanupCancel:  cleanupCancel,
	}
}

func (d *delayer) NewTask(task func(context.Context, bool) error) DelayedTask {
	id := atomic.AddInt64(&d.counter, 1)

	return &delayedTask{
		delayer: d,
		logger:  d.logger.WithValues("taskId", id),
		Id:      DelayedTaskId(id),
		task:    task,
	}
}

func (d *delayer) Stop(drain time.Duration, cleanup time.Duration) {
	d.logger.Info("Stopping delayer")

	stopped := make(chan struct{}, 1)
	go func() {
		d.tasksWaitGroup.Wait()
		stopped <- struct{}{}
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
	d.cleanupCancel()
	d.logger.Info("Stopped delayer")
}

type DelayedTask interface {
	WithLoggerValues(keysAndValues ...interface{}) DelayedTask
	RunAfterWait(ctx context.Context, duration time.Duration) error
	RunAfterAsync(duration time.Duration)
}

var _ DelayedTask = &delayedTask{}

type delayedTask struct {
	delayer *delayer
	logger  logr.Logger
	Id      DelayedTaskId
	task    func(context.Context, bool) error
}

type DelayedTaskId int64

func (t *delayedTask) WithLoggerValues(keysAndValues ...interface{}) DelayedTask {
	return &delayedTask{
		delayer: t.delayer,
		logger:  t.logger.WithValues(keysAndValues...),
		Id:      t.Id,
		task:    t.task,
	}
}

func (t *delayedTask) RunAfterWait(ctx context.Context, duration time.Duration) error {
	t.delayer.tasksWaitGroup.Add(1)
	defer t.delayer.tasksWaitGroup.Done()

	innerCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	go func() {
		select {
		case <-innerCtx.Done():
		case <-t.delayer.cleanupContext.Done():
			cancel()
		}
	}()

	return t.run(innerCtx, duration)
}

func (t *delayedTask) RunAfterAsync(duration time.Duration) {
	t.delayer.tasksWaitGroup.Add(1)
	ctx, cancel := context.WithCancel(t.delayer.cleanupContext)

	go func() {
		defer t.delayer.tasksWaitGroup.Done()
		defer cancel()

		err := t.run(ctx, duration)
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
		newCtx := context.WithValue(ctx, delayedTaskScopeLoggerKey, t.logger)

		if err := t.task(newCtx, interrupted); err != nil {
			t.logger.Error(err, "Delayed task errored")
			return err
		}
	}
	return nil
}

const (
	delayedTaskScopeLoggerKey = "DelayedTaskScopeLoggerKey"
)

func GetTaskScopedLogger(ctx context.Context) logr.Logger {
	return ctx.Value(delayedTaskScopeLoggerKey).(logr.Logger)
}
