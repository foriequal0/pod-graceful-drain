module github.com/foriequal0/pod-graceful-drain

go 1.13

require (
	github.com/go-logr/logr v0.1.0
	github.com/golang/protobuf v1.4.2
	github.com/pkg/errors v0.9.1
	go.uber.org/zap v1.10.0
	gonum.org/v1/netlib v0.0.0-20190331212654-76723241ea4e // indirect
	gotest.tools v2.2.0+incompatible
	k8s.io/api v0.18.6
	k8s.io/apimachinery v0.18.6
	k8s.io/client-go v0.18.6
	k8s.io/kubectl v0.18.0
	k8s.io/kubernetes v1.13.0
	sigs.k8s.io/aws-load-balancer-controller v0.0.0
	sigs.k8s.io/controller-runtime v0.6.3
	sigs.k8s.io/structured-merge-diff v1.0.1-0.20191108220359-b1b620dd3f06 // indirect
)

replace sigs.k8s.io/aws-load-balancer-controller => ./forks/sigs.k8s.io/aws-load-balancer-controller@v2.1.1
