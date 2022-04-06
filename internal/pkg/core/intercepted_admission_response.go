package core

import (
	"encoding/json"
	"github.com/pkg/errors"
	"gomodules.xyz/jsonpatch/v2"
	policyv1 "k8s.io/api/policy/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/controller-runtime/pkg/webhook/admission"
)

type InterceptedAdmissionResponse interface {
	GetAdmissionResponse() admission.Response
}

type AdmissionResponse struct {
	Allow  bool
	Reason string
}

func (r AdmissionResponse) GetAdmissionResponse() admission.Response {
	if r.Allow {
		return admission.Allowed(r.Reason)
	}
	return admission.Denied(r.Reason)
}

type EvictionResponse struct {
	Operations []jsonpatch.Operation
}

func NewEvictionResponse(eviction *policyv1.Eviction) (EvictionResponse, error) {
	oldJson, err := json.Marshal(eviction)
	if err != nil {
		return EvictionResponse{}, errors.Wrap(err, "unable to marshal old eviction")
	}
	modified := eviction.DeepCopy()
	if modified.DeleteOptions == nil {
		modified.DeleteOptions = &metav1.DeleteOptions{}
	}
	modified.DeleteOptions.DryRun = append(modified.DeleteOptions.DryRun, "All")
	modifiedJson, err := json.Marshal(modified)
	if err != nil {
		return EvictionResponse{}, errors.Wrap(err, "unable to marshal old eviction")
	}
	operations, err := jsonpatch.CreatePatch(oldJson, modifiedJson)
	if err != nil {
		return EvictionResponse{}, errors.Wrap(err, "unable to create jsonpatch")
	}
	return EvictionResponse{Operations: operations}, nil
}

func (r EvictionResponse) GetAdmissionResponse() admission.Response {
	return admission.Patched("Pod eviction is intercepted", r.Operations...)
}
