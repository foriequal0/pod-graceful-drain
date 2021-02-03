package internal

import (
	"flag"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"k8s.io/apimachinery/pkg/runtime"
	ctrl "sigs.k8s.io/controller-runtime"
)

type Config struct {
	LogLevel           string
	MetricsBindAddress string
	WebhookBindPort    int

	PodGracefulDrain core.PodGracefulDrainConfig
}

func (c *Config) BindFlags(fs *flag.FlagSet) {
	fs.StringVar(&c.LogLevel, "log-level", "info", "Log level: info, debug")
	fs.StringVar(&c.MetricsBindAddress, "metrics-bind-address", ":8080", "The address the metric endpoint binds to.")
	fs.IntVar(&c.WebhookBindPort, "webhook-bind-port", 9443, "The port the webhook server serves at.")

	c.PodGracefulDrain.BindFlags(fs)
}

func (c *Config) BuildManagerOptions(scheme *runtime.Scheme) ctrl.Options {
	return ctrl.Options{
		Scheme:             scheme,
		MetricsBindAddress: c.MetricsBindAddress,
		Port:               c.WebhookBindPort,
	}
}

func (c *Config) Validate() error {
	if err := c.PodGracefulDrain.Validate(); err != nil {
		return err
	}

	return nil
}
