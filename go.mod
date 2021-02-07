module github.com/foriequal0/pod-graceful-drain

go 1.13

require (
	github.com/go-logr/logr v0.3.0
	github.com/pkg/errors v0.9.1
	go.uber.org/zap v1.15.0
	gomodules.xyz/jsonpatch/v2 v2.1.0
	gotest.tools v2.2.0+incompatible
	k8s.io/api v0.20.2
	k8s.io/apimachinery v0.20.2
	k8s.io/client-go v0.20.2
	k8s.io/kubernetes v1.13.0
	sigs.k8s.io/aws-load-balancer-controller v0.0.0
	sigs.k8s.io/controller-runtime v0.8.1
)

replace sigs.k8s.io/aws-load-balancer-controller => ./forks/sigs.k8s.io/aws-load-balancer-controller@v2.1.1
