import time
from datetime import datetime, timedelta
from pathlib import Path

from utils.kubectl import (
    kubectl,
    pod_is_alive,
    kubectl_nowait,
    kubectl_stdin,
    KubectlContext,
)
from utils.kind import KindContext


# prerequisite:
# > kind create cluster --name test-pgd --kubeconfig test-pgd.kubeconfig
# > skaffold run --kubeconfig test-pgd.kubeconfig --tag latest


def test_can_delete_pod_without_delay_if_no_ingress(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(kind_ctx.get_kubeconfig(), namespace=None) as context:
        kubectl(
            context,
            "run",
            "busybox-sleep",
            "--image=public.ecr.aws/docker/library/busybox",
            "--",
            "sleep",
            "1000",
        )
        kubectl(context, "wait", "pod/busybox-sleep", "--for=condition=Ready")
        start = datetime.now()
        kubectl(context, "delete", "pod/busybox-sleep", "--wait=false")
        diff = datetime.now() - start
        assert diff < timedelta(seconds=10), "it should be quick"
        assert not pod_is_alive(context, "pod/busybox-sleep")


def test_delete_is_delayed_with_ingress(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(kind_ctx.get_kubeconfig(), namespace=None) as context:
        kubectl_stdin(
            context,
            "apply",
            "-f",
            "-",
            stdin="""
apiVersion: v1
kind: Pod
metadata:
  name: some-pod
  labels:
    app: test
spec:
  containers:
  - name: app
    image: public.ecr.aws/docker/library/busybox
    command: ["sleep", "9999"]
---
apiVersion: v1
kind: Service
metadata:
  name: some-service
spec:
  ports:
  - name: http
    port: 80
  selector:
    app: test
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: some-service
            port:
              name: http
        """,
        )
        kubectl(context, "wait", "pod/some-pod", "--for=condition=Ready")

        time.sleep(1)  # give some time to settle down

        kubectl_nowait(context, "delete", "pod/some-pod", "--wait=false")

        for secs in range(0, 20 - 5):
            assert pod_is_alive(
                context, "pod/some-pod"
            ), f"pod should be alive for approx. 20s, but died in {secs}s"
            time.sleep(1)
