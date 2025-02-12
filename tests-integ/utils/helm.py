import os
import subprocess

from .kubectl import KubectlContext
from .subprocess_util import handle_error, print_command


def _get_command(ctx: KubectlContext, /, *args):
    command = ["helm"]

    if ctx.kubeconfig is not None:
        command.extend(["--kubeconfig", ctx.kubeconfig])

    command.extend(["--namespace", ctx.namespace])

    command.extend(args)
    return command


def helm(kubectl_ctx: KubectlContext, /, *args):
    command = _get_command(kubectl_ctx, *args)
    print_command(command)
    result = subprocess.run(command, capture_output=True)
    handle_error(result)


def helm_install(
    kubectl_ctx: KubectlContext,
    repository: str | None = None,
    tag: str | None = None,
    values: dict[str, str] | None = None,
):
    repo = repository if repository else "localhost/pod-graceful-drain"
    tag = tag if tag else "latest"

    set_values_args = []
    for key, value in values.items():
        set_values_args.append("--set")
        set_values_args.append(f"{key}={value}")

    proj_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
    helm(
        kubectl_ctx,
        "install",
        "pod-graceful-drain",
        os.path.join(proj_dir, "charts/pod-graceful-drain"),
        "--create-namespace",
        "--set",
        f"image.repository={repo}",
        "--set",
        f"image.tag={tag}",
        "--set",
        "experimentalGeneralIngress=true",
        "--set",
        "logLevel=info\\,pod_graceful_drain=trace",
        *set_values_args,
        "--wait=true",
        "--timeout=1m",
    )
