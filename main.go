/*


Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package main

import (
	"flag"
	"github.com/foriequal0/pod-graceful-drain/internal"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/webhooks"
	"github.com/go-logr/logr"
	"github.com/pkg/errors"
	"go.uber.org/zap/zapcore"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	_ "k8s.io/client-go/plugin/pkg/client/auth/gcp"
	"os"
	elbv2api "sigs.k8s.io/aws-load-balancer-controller/apis/elbv2/v1beta1"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	// +kubebuilder:scaffold:imports
)

var (
	scheme   = runtime.NewScheme()
	setupLog = ctrl.Log.WithName("setup")

	GitVersion string
	GitCommit  string
	BuildDate  string
)

func init() {
	_ = clientgoscheme.AddToScheme(scheme)
	_ = elbv2api.AddToScheme(scheme)
	// +kubebuilder:scaffold:scheme
}

func main() {
	cfg, err := parseConfig()
	if err != nil {
		setupLog.Error(err, "unable to parse controller config")
		os.Exit(1)
	}

	logger, err := createLogger(cfg.LogLevel)
	if err != nil {
		setupLog.Error(err, "unable to create logger")
		os.Exit(1)
	}
	ctrl.SetLogger(logger)

	setupLog.Info("version",
		"GitVersion", GitVersion,
		"GitCommit", GitCommit,
		"BuildDate", BuildDate,
	)

	mgr, err := ctrl.NewManager(ctrl.GetConfigOrDie(), cfg.BuildManagerOptions(scheme))
	if err != nil {
		setupLog.Error(err, "unable to start manager")
		os.Exit(1)
	}

	// +kubebuilder:scaffold:builder

	drain := core.NewPodGracefulDrain(mgr.GetClient(), ctrl.Log, &cfg.PodGracefulDrain)
	if err := mgr.Add(&drain); err != nil {
		setupLog.Error(err, "unable to setup pod-graceful-drain")
		os.Exit(1)
	}
	interceptor := core.NewInterceptor(&drain, mgr.GetClient())

	podValidationWebhook := webhooks.NewPodValidator(&interceptor, ctrl.Log, &cfg.PodGracefulDrain)
	if err := podValidationWebhook.SetupWebhookWithManager(mgr); err != nil {
		setupLog.Error(err, "unable to create webhook", "webhook", "pod-validation-webhook")
		os.Exit(1)
	}

	evictionValidationWebhook := webhooks.NewEvictionValidator(&interceptor, ctrl.Log, &cfg.PodGracefulDrain)
	if err := evictionValidationWebhook.SetupWebhookWithManager(mgr); err != nil {
		setupLog.Error(err, "unable to create webhook", "webhook", "pod-eviction-validation-webhook")
		os.Exit(1)
	}

	setupLog.Info("starting manager")
	if err := mgr.Start(ctrl.SetupSignalHandler()); err != nil {
		setupLog.Error(err, "problem running manager")
		os.Exit(1)
	}
}

func parseConfig() (internal.Config, error) {
	fs := flag.NewFlagSet("", flag.ExitOnError)
	cfg := internal.Config{}
	cfg.BindFlags(fs)
	err := fs.Parse(os.Args)
	return cfg, err
}

func createLogger(logLevel string) (logr.Logger, error) {
	var zapcoreLevel zapcore.Level
	switch logLevel {
	case "info":
		zapcoreLevel = zapcore.InfoLevel
	case "debug":
		zapcoreLevel = zapcore.DebugLevel
	default:
		return logr.Logger{}, errors.New("Invalid log level")
	}

	logger := zap.New(zap.UseDevMode(false),
		zap.Level(zapcoreLevel),
		zap.StacktraceLevel(zapcore.FatalLevel))
	return logger, nil
}
