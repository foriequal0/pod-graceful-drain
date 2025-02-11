import time
from datetime import datetime, timedelta
from pathlib import Path

from utils.helm import helm_install
from utils.kind import KindContext, kind
from utils.kubectl import (
    kubectl,
    kubectl_stdin,
    KubectlContext,
    pod_is_alive,
    kubectl_nowait,
)


# prerequisite:
# > skaffold build --tag latest


def test_drain_self(tmp_path: Path):
    with (
        KindContext(tmp_path, cluster_name=None, workers=2) as kind_ctx,
        KubectlContext(
            kind_ctx.get_kubeconfig(), namespace="pod-graceful-drain"
        ) as kubectl_ctx,
    ):
        kind(
            kind_ctx,
            "load",
            "docker-image",
            "localhost/pod-graceful-drain:latest",
            "--name",
            kind_ctx.cluster_name,
        )

        helm_install(kubectl_ctx)

        kubectl_stdin(
            kubectl_ctx,
            "apply",
            "-f",
            "-",
            stdin=f"""
apiVersion: v1
kind: Pod
metadata:
  name: some-pod
  labels:
    app: test
spec:
  nodeName: "{kind_ctx.cluster_name}-worker2"
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
      - backend:
          service:
            name: some-service
            port:
              name: http
        pathType: Exact
        path: /
        """,
        )
        kubectl(kubectl_ctx, "wait", "pod/some-pod", "--for=condition=Ready")

        start = datetime.now()
        kubectl(
            kubectl_ctx,
            "drain",
            "--force",
            "--ignore-daemonsets",
            f"{kind_ctx.cluster_name}-worker",
        )
        diff = datetime.now() - start
        assert diff < timedelta(seconds=10 + 5), "it should be quick"
        assert pod_is_alive(kubectl_ctx, "pod/some-pod")


def test_drain_other(tmp_path: Path):
    with (
        KindContext(tmp_path, cluster_name=None, workers=2) as kind_ctx,
        KubectlContext(
            kind_ctx.get_kubeconfig(), namespace="pod-graceful-drain"
        ) as kubectl_ctx,
    ):
        kind(
            kind_ctx,
            "load",
            "docker-image",
            "localhost/pod-graceful-drain:latest",
            "--name",
            kind_ctx.cluster_name,
        )

        helm_install(kubectl_ctx)

        # forcefully place some-pod to worker2
        kubectl_stdin(
            kubectl_ctx,
            "apply",
            "-f",
            "-",
            stdin=f"""
apiVersion: v1
kind: Pod
metadata:
  name: some-pod
  labels:
    app: test
spec:
  nodeName: "{kind_ctx.cluster_name}-worker2"
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
      - backend:
          service:
            name: some-service
            port:
              name: http
        pathType: Exact
        path: /
        """,
        )
        kubectl(kubectl_ctx, "wait", "pod/some-pod", "--for=condition=Ready")

        kubectl_nowait(
            kubectl_ctx,
            "drain",
            "--force",
            "--ignore-daemonsets",
            f"{kind_ctx.cluster_name}-worker2",
        )

        for _ in range(0, 20 - 5):
            assert pod_is_alive(
                kubectl_ctx, "pod/some-pod"
            ), "pod should be alive for approx. 20s"
            time.sleep(1)
