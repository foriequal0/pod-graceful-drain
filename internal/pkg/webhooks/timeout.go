package webhooks

import (
	"context"
	"net/http"
	ctrl "sigs.k8s.io/controller-runtime"
	"time"
)

const (
	timeoutContextKey     = "timeoutContextKey"
	webhookDefaultTimeout = 10 * time.Second
)

func WithTimeoutContext(ctx context.Context, req *http.Request) context.Context {
	query := req.URL.Query()
	timeout := query.Get("timeout")
	if len(timeout) == 0 {
		return ctx
	}
	duration, err := time.ParseDuration(timeout)
	if err != nil {
		ctrl.Log.Error(err, "unable to parse timeout")
	}

	return context.WithValue(ctx, timeoutContextKey, &duration)
}

func WithTimeout(ctx context.Context) (context.Context, context.CancelFunc) {
	timeout := ctx.Value(timeoutContextKey).(*time.Duration)
	if timeout != nil {
		return context.WithTimeout(ctx, *timeout)
	} else {
		return context.WithTimeout(ctx, webhookDefaultTimeout)
	}
}
