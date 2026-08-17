{{- define "un1c0.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "un1c0.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name (include "un1c0.name" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{- define "un1c0.labels" -}}
app.kubernetes.io/name: {{ include "un1c0.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
{{- end }}

{{- define "un1c0.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "un1c0.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "un1c0.adminImage" -}}
{{- if .Values.admin.image.digest }}{{ .Values.admin.image.repository }}@{{ .Values.admin.image.digest }}{{ else }}{{ .Values.admin.image.repository }}:{{ .Values.admin.image.tag }}{{ end }}
{{- end }}

{{- define "un1c0.nginxImage" -}}
{{- if .Values.nginx.image.digest }}{{ .Values.nginx.image.repository }}@{{ .Values.nginx.image.digest }}{{ else }}{{ .Values.nginx.image.repository }}:{{ .Values.nginx.image.tag }}{{ end }}
{{- end }}
