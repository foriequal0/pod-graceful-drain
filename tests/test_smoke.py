import subprocess
import sys
from datetime import datetime, timedelta
import random
import time

namespace = ""


def setup_module():
    global namespace
    namespace = f"pgd-test-{random.randrange(10000, 99999)}"
    kubectl("create", "namespace", namespace)
    kubectl("label", "namespace", namespace, "test=true")
    print("testing on namespace: ", namespace)


def teardown_module():
    global namespace
    kubectl("delete", "namespace", namespace)


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def kubectl(*args):
    global namespace

    result = subprocess.run(
        ["kubectl", "--namespace", namespace, *args],
        capture_output=True)

    if result.returncode != 0:
        eprint("stdout:")
        eprint(result.stdout)
        eprint("stderr:")
        eprint(result.stderr)
        raise Exception(f"'kubectl {" ".join(args)}' failed with exit code '{result.returncode}'")


def kubectl_stdin(args, /, input):
    global namespace

    result = subprocess.run(
        ["kubectl", "--namespace", namespace, *args],
        capture_output=True,
        input=input, encoding="utf-8")

    if result.returncode != 0:
        eprint("stdout:")
        eprint(result.stdout)
        eprint("stderr:")
        eprint(result.stderr)
        raise Exception(f"'kubectl {" ".join(args)}' failed with exit code '{result.returncode}'")


def kubectl_nowait(args):
    global namespace

    child = subprocess.Popen(
        ["kubectl", "--namespace", namespace, *args])

    return child


def pod_is_alive(name):
    global namespace

    result = subprocess.run(
        ["kubectl", "--namespace", namespace, "get", name, "-o", "jsonpath={.metadata.deletionTimestamp}"],
        capture_output=True, encoding="utf-8")

    if result.returncode != 0:
        return False

    stdout = result.stdout.strip()
    return not stdout


def test_can_delete_pod_without_delay_if_no_ingress():
    kubectl("run", "busybox-sleep", "--image=public.ecr.aws/docker/library/busybox", "--", "sleep", "1000")
    kubectl("wait", "pod/busybox-sleep", "--for=condition=Ready")
    start = datetime.now()
    kubectl("delete", "pod/busybox-sleep", "--wait=false")
    diff = datetime.now() - start
    assert diff < timedelta(seconds=10), "it should be quick"
    assert not pod_is_alive("pod/busybox-sleep")


def test_delete_is_delayed_with_ingress():
    kubectl("run", "nginx", "--image=nginx")
    kubectl("expose", "pod", "nginx", "--port=80", "--target-port=8000")
    kubectl_stdin(["apply", "-f", "-"], input="""
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: nginx
spec:
  rules:
  - http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: nginx
            port:
              number: 80
    """)
    kubectl("wait", "pod/nginx", "--for=condition=Ready")

    time.sleep(1)  # give some time to settle down

    kubectl_nowait(["delete", "pod/nginx", "--wait=false"])

    for _ in range(0, 20 - 5):
        assert pod_is_alive("pod/nginx"), "pod should be alive for approx. 20s"
        time.sleep(1)
