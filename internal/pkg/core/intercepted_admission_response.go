package core

import "sigs.k8s.io/controller-runtime/pkg/webhook/admission"

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
