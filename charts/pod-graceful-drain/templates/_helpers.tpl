{{/*
Expand the name of the chart.
*/}}
{{- define "pod-graceful-drain.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "pod-graceful-drain.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "pod-graceful-drain.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "pod-graceful-drain.labels" -}}
helm.sh/chart: {{ include "pod-graceful-drain.chart" . }}
{{ include "pod-graceful-drain.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "pod-graceful-drain.selectorLabels" -}}
app.kubernetes.io/name: {{ include "pod-graceful-drain.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "pod-graceful-drain.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "pod-graceful-drain.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Generate certificates for webhook
*/}}
{{- define "pod-graceful-drain.gen-certs" -}}
{{- $fullname := ( include "pod-graceful-drain.fullname" . ) -}}
{{- $altNames := list ( printf "%s-%s.%s" $fullname "webhook-service" .Release.Namespace ) ( printf "%s-%s.%s.svc" $fullname "webhook-service" .Release.Namespace ) -}}
{{- $ca := genCA "pod-graceful-drain-ca" 3650 -}}
{{- $cert := genSignedCert ( include "pod-graceful-drain.fullname" . ) nil $altNames 3650 $ca -}}
caCert: {{ $ca.Cert | b64enc }}
clientCert: {{ $cert.Cert | b64enc }}
clientKey: {{ $cert.Key | b64enc }}
{{- end -}}

{{/*
Timeouts: +5s to deleteAfter
*/}}
{{- define "pod-graceful-drain.timeoutSeconds" -}}
{{- $now := now -}}
{{- $seconds := sub ($now | dateModify .Values.deleteAfter | unixEpoch) ($now | unixEpoch) -}}
{{- printf "%d" (add $seconds 5) -}}
{{- end }}
