import random
import string
import subprocess
from time import sleep

from .subprocess_util import handle_error, print_command, to_bytes


class KubectlContext:
    def __init__(self, kubeconfig: str, /, namespace: str | None):
        self.kubeconfig = kubeconfig
        self.namespace = namespace if namespace else _random_name()

    def __enter__(self):
        kubectl(self, "create", "namespace", self.namespace)
        kubectl(self, "label", "namespace", self.namespace, "test=true")
        # it takes some time to fully create some resources
        sleep(1)

        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is not None:
            # dump logs
            kubectl(
                self, "logs", "deployment/pod-graceful-drain", "--all-pods", "--prefix"
            )

            return

        kubectl(self, "delete", "namespace", self.namespace, "--ignore-not-found")


def _get_command(context: KubectlContext, /, *args):
    command = ["kubectl"]

    if context.kubeconfig is not None:
        command.extend(["--kubeconfig", context.kubeconfig])

    command.extend(["--namespace", context.namespace])
    command.extend(args)

    return command


def kubectl(context: KubectlContext, /, *args):
    command = _get_command(context, *args)
    print_command(command)
    result = subprocess.run(command)
    handle_error(result)


def kubectl_stdin(context: KubectlContext, /, *args, stdin):
    command = _get_command(context, *args)
    print_command([*command, f"<<<EOF\n{stdin}\nEOF"])
    result = subprocess.run(command, input=to_bytes(stdin))
    handle_error(result)


def kubectl_nowait(context: KubectlContext, /, *args):
    command = _get_command(context, *args)
    print_command(command)
    child = subprocess.Popen(command)
    return child


def pod_is_annotated(context: KubectlContext, name):
    command = _get_command(
        context,
        "get",
        name,
        "-o",
        "jsonpath={.metadata.labels['pod-graceful-drain/draining']}",
    )
    print_command(command)
    result = subprocess.run(command, capture_output=True, encoding="utf-8")

    if result.returncode != 0:
        return False

    stdout = result.stdout.strip()
    return stdout


def pod_is_alive(context: KubectlContext, name):
    command = _get_command(
        context, "get", name, "-o", "jsonpath={.metadata.deletionTimestamp}"
    )
    print_command(command)
    result = subprocess.run(command, capture_output=True, encoding="utf-8")

    if result.returncode != 0:
        return False

    stdout = result.stdout.strip()
    return not stdout


def _random_name():
    alnum = string.ascii_lowercase + string.digits
    suffix = "".join(random.choice(alnum) for _ in range(8))
    return "test-pgd-" + suffix
