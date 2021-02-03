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

package webhooks

import (
	"context"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/core"
	"github.com/foriequal0/pod-graceful-drain/internal/pkg/interceptors"
	"github.com/go-logr/logr"
	admissionv1beta1 "k8s.io/api/admission/v1beta1"
	"k8s.io/api/policy/v1beta1"
	"k8s.io/apimachinery/pkg/types"
	"net/http"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

var _ admission.DecoderInjector = &PodValidator{}
var _ admission.Handler = &PodValidator{}

type EvictionValidator struct {
	interceptor interceptors.PodEvictionInterceptor
	logger      logr.Logger
	config      *core.PodGracefulDrainConfig

	decoder *admission.Decoder
}

func NewEvictionValidator(interceptor interceptors.PodEvictionInterceptor, logger logr.Logger, config *core.PodGracefulDrainConfig) EvictionValidator {
	return EvictionValidator{
		interceptor: interceptor,
		logger:      logger.WithName("pod-eviction-validation-webhook"),
		config:      config,
	}
}

func (v *EvictionValidator) InjectDecoder(decoder *admission.Decoder) error {
	v.decoder = decoder
	return nil
}

func (v *EvictionValidator) Handle(ctx context.Context, req admission.Request) admission.Response {
	switch req.Operation {
	case admissionv1beta1.Create:
		return v.handleCreate(ctx, req)
	default:
		return admission.Allowed("")
	}
}

func (v *EvictionValidator) handleCreate(ctx context.Context, req admission.Request) admission.Response {
	eviction := v1beta1.Eviction{}
	if err := v.decoder.DecodeRaw(req.OldObject, &eviction); err != nil {
		return admission.Errored(http.StatusBadRequest, err)
	}

	logger := v.logger.WithValues("eviction", types.NamespacedName{Namespace: eviction.Namespace, Name: eviction.Name})

	handler, err := v.interceptor.Intercept(ctx, &req, &eviction)
	if err != nil {
		logger.Error(err, "errored while intercepting pod eviction")
		if v.config.IgnoreError {
			return admission.Allowed("ignore error during intercepting pod deletion")
		} else {
			return admission.Errored(1, err)
		}
	} else if handler != nil {
		return handler.HandleInterceptedAdmission()
	}

	return admission.Allowed("")
}

// +kubebuilder:webhook:verbs=create,path=/validate-policy-v1beta1-eviction,mutating=false,failurePolicy=ignore,groups=policy,resources=pods/eviction,versions=v1beta1,name=vpodseviction.pod-graceful-drain.io

func (v *EvictionValidator) SetupWebhookWithManager(mgr ctrl.Manager) error {
	mgr.GetWebhookServer().Register("/validate-policy-v1beta1-eviction", &admission.Webhook{
		Handler: v,
	})
	return nil
}
