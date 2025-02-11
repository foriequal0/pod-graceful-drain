import random
import string
import subprocess
import sys
from pathlib import Path

from .subprocess_util import handle_error, print_command, to_bytes

DEFAULT_KIND_IMAGE = "kindest/node:v1.32.0"


class KindContext:
    def __init__(self, tmp_path: Path, /, cluster_name: str | None, workers: int = 1):
        self.cluster_name = cluster_name if cluster_name else _random_name()
        self._kubeconfig = str(tmp_path / "kubeconfig")
        self._workers = workers

    def create_cluster(self):
        if self._workers > 1:
            config = """
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
- role: control-plane
"""
            for _ in range(self._workers):
                config += "- role: worker\n"

            kind_stdin(
                self,
                "create",
                "cluster",
                "--image",
                DEFAULT_KIND_IMAGE,
                "--name",
                self.cluster_name,
                # not to messing with global kubeconfig.
                "--kubeconfig",
                self._kubeconfig,
                # from stdin
                "--config",
                "-",
                stdin=config,
            )
        else:
            kind(
                self,
                "create",
                "cluster",
                "--image",
                DEFAULT_KIND_IMAGE,
                "--name",
                self.cluster_name,
                # not to messing with global kubeconfig.
                "--kubeconfig",
                self._kubeconfig,
            )

    def cleanup_cluster(self):
        kind(self, "delete", "cluster", "--name", self.cluster_name)

    def get_kubeconfig(self):
        kubeconfig = kind_stdout(self, "get", "kubeconfig", "--name", self.cluster_name)
        with open(self._kubeconfig, "wb") as f:
            f.write(kubeconfig)

        return self._kubeconfig

    def __enter__(self):
        self.create_cluster()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.cleanup_cluster()
        pass


def _get_command(_ctx: KindContext, /, *args):
    command = ["kind"]
    command.extend(args)
    return command


def kind(ctx: KindContext, /, *args):
    command = _get_command(ctx, *args)
    print_command(command)
    result = subprocess.run(command)
    handle_error(result)


def kind_stdin(ctx: KindContext, /, *args, stdin):
    command = _get_command(ctx, *args)
    print_command([*command, f"<<<EOF\n{stdin}\nEOF"])
    result = subprocess.run(command, input=to_bytes(stdin))
    handle_error(result)


def kind_stdout(ctx: KindContext, /, *args):
    command = _get_command(ctx, *args)
    print_command(command)
    result = subprocess.run(command, capture_output=True)
    if result.returncode != 0:
        sys.stderr.write(result.stderr.decode("utf-8"))
        raise Exception(f"Command failed with exit code '{result.returncode}'")

    return result.stdout


def _random_name():
    alnum = string.ascii_lowercase + string.digits
    suffix = "".join(random.choice(alnum) for _ in range(8))
    return "test-pgd-" + suffix
