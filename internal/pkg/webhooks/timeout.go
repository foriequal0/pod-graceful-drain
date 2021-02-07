package webhooks

import (
	"context"
	"net/http"
	ctrl "sigs.k8s.io/controller-runtime"
	"time"
)

const (
	webhookDefaultTimeout = 10 * time.Second
)

type contextKey struct{}

func NewContextFromRequest(ctx context.Context, req *http.Request) context.Context {
	query := req.URL.Query()
	timeout := query.Get("timeout")
	if len(timeout) == 0 {
		return ctx
	}
	duration, err := time.ParseDuration(timeout)
	if err != nil {
		ctrl.Log.Error(err, "unable to parse timeout")
	}

	return context.WithValue(ctx, contextKey{}, &duration)
}

func TimeoutFromContext(ctx context.Context) time.Duration {
	timeout := ctx.Value(contextKey{}).(*time.Duration)
	if timeout != nil {
		return *timeout
	} else {
		return webhookDefaultTimeout
	}
}
