import time
from datetime import datetime, timedelta
from pathlib import Path

from utils.kubectl import (
    kubectl,
    pod_is_alive,
    kubectl_nowait,
    kubectl_stdin,
    KubectlContext,
    pod_is_annotated,
)
from utils.kind import KindContext


# prerequisite:
# > kind create cluster --name test-pgd --kubeconfig test-pgd.kubeconfig
# > skaffold run --kubeconfig test-pgd.kubeconfig --tag latest


def test_should_delete_pod_without_delay_if_no_ingress(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(
        kind_ctx.get_kubeconfig(), namespace=None, create_namespace=True
    ) as kubectl_ctx:
        kubectl(
            kubectl_ctx,
            "run",
            "busybox-sleep",
            "--image=public.ecr.aws/docker/library/busybox",
            "--",
            "sleep",
            "1000",
        )
        kubectl(kubectl_ctx, "wait", "pod/busybox-sleep", "--for=condition=Ready")

        start = datetime.now()
        kubectl(
            kubectl_ctx,
            "delete",
            "pod/busybox-sleep",
            "--wait=False",  # do not wait for the cleanup
        )
        diff = datetime.now() - start
        assert diff < timedelta(seconds=10), "it should be quick"
        assert not pod_is_alive(kubectl_ctx, "pod/busybox-sleep")


def test_should_evict_pod_without_delay_if_no_ingress(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(
        kind_ctx.get_kubeconfig(), namespace=None, create_namespace=True
    ) as kubectl_ctx:
        kubectl(
            kubectl_ctx,
            "run",
            "busybox-sleep",
            "--image=public.ecr.aws/docker/library/busybox",
            "--",
            "sleep",
            "1000",
        )
        kubectl(kubectl_ctx, "wait", "pod/busybox-sleep", "--for=condition=Ready")

        kubectl_stdin(
            kubectl_ctx,
            "create",
            f"--raw=/api/v1/namespaces/{kubectl_ctx.namespace}/pods/busybox-sleep/eviction",
            "-f",
            "-",
            stdin=f"""
{{
  "apiVersion": "policy/v1",
  "kind": "Eviction",
  "metadata": {{
    "name": "busybox-sleep",
    "namespace": "{kubectl_ctx.namespace}"
  }}
}}
        """,
        )

        time.sleep(1)
        assert not pod_is_alive(kubectl_ctx, "pod/busybox-sleep")


def test_delete_is_delayed_with_ingress(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(
        kind_ctx.get_kubeconfig(), namespace=None, create_namespace=True
    ) as kubectl_ctx:
        kubectl_stdin(
            kubectl_ctx,
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
        kubectl(kubectl_ctx, "wait", "pod/some-pod", "--for=condition=Ready")

        kubectl_nowait(kubectl_ctx, "delete", "pod/some-pod")

        time.sleep(1)  # give some time to process
        assert (
            pod_is_annotated(kubectl_ctx, "pod/some-pod") == "true"
        ), "pod is annotated"
        for secs in range(0, 20 - 5):
            assert pod_is_alive(
                kubectl_ctx, "pod/some-pod"
            ), f"pod should be alive for approx. 20s, but died in {secs}s"
            time.sleep(1)

        time.sleep(10)
        assert not pod_is_alive(
            kubectl_ctx, "pod/some-pod"
        ), "pod should be dead by now"


def test_eviction_is_delayed_with_ingress(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(
        kind_ctx.get_kubeconfig(), namespace=None, create_namespace=True
    ) as kubectl_ctx:
        kubectl_stdin(
            kubectl_ctx,
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
        kubectl(kubectl_ctx, "wait", "pod/some-pod", "--for=condition=Ready")

        kubectl_stdin(
            kubectl_ctx,
            "create",
            f"--raw=/api/v1/namespaces/{kubectl_ctx.namespace}/pods/some-pod/eviction",
            "-f",
            "-",
            stdin=f"""
{{
  "apiVersion": "policy/v1",
  "kind": "Eviction",
  "metadata": {{
    "name": "some-pod",
    "namespace": "{kubectl_ctx.namespace}"
  }}
}}
        """,
        )

        time.sleep(1)  # give some time to process
        assert (
            pod_is_annotated(kubectl_ctx, "pod/some-pod") == "true"
        ), "pod is annotated"
        for secs in range(0, 20 - 5):
            assert pod_is_alive(
                kubectl_ctx, "pod/some-pod"
            ), f"pod should be alive for approx. 20s, but died in {secs}s"
            time.sleep(1)

        time.sleep(10)
        assert not pod_is_alive(
            kubectl_ctx, "pod/some-pod"
        ), "pod should be dead by now"


def test_eviction_is_further_delayed_with_pdb(tmp_path: Path):
    kind_ctx = KindContext(tmp_path, cluster_name="test-pgd")
    with KubectlContext(
        kind_ctx.get_kubeconfig(), namespace=None, create_namespace=True
    ) as kubectl_ctx:
        kubectl_stdin(
            kubectl_ctx,
            "apply",
            "-f",
            "-",
            stdin="""
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: some-pdb
spec:
  selector:
    matchLabels:
      app: test
  minAvailable: 1
---
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
        kubectl(kubectl_ctx, "wait", "pod/some-pod", "--for=condition=Ready")
        time.sleep(1)  # give some time to process

        try:
            kubectl_stdin(
                kubectl_ctx,
                "create",
                f"--raw=/api/v1/namespaces/{kubectl_ctx.namespace}/pods/some-pod/eviction",
                "-f",
                "-",
                stdin=f"""
    {{
      "apiVersion": "policy/v1",
      "kind": "Eviction",
      "metadata": {{
        "name": "some-pod",
        "namespace": "{kubectl_ctx.namespace}"
      }}
    }}
            """,
            )
        except Exception:
            pass  # it can raise exception

        for secs in range(0, 5):
            time.sleep(1)
            assert (
                pod_is_annotated(kubectl_ctx, "pod/some-pod") == "evicting"
            ), "pod should be in waiting state"

        kubectl_stdin(
            kubectl_ctx,
            "apply",
            "-f",
            "-",
            stdin="""
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: some-pdb
spec:
  selector:
    matchLabels:
      app: test
  minAvailable: 0
        """,
        )

        time.sleep(3)  # give some time to process
        assert (
            pod_is_annotated(kubectl_ctx, "pod/some-pod") == "true"
        ), "pod is draining"

        for secs in range(0, 20 - 5):
            assert pod_is_alive(
                kubectl_ctx, "pod/some-pod"
            ), f"pod should be alive for approx. 20s, but died in {secs}s"
            time.sleep(1)

        time.sleep(10)
        assert not pod_is_alive(
            kubectl_ctx, "pod/some-pod"
        ), "pod should be dead by now"
