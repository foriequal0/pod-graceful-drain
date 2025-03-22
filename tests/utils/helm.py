import os
import subprocess

from .kubectl import KubectlContext
from .subprocess_util import handle_error, print_command


def _get_command(ctx: KubectlContext, /, *args):
    command = ["helm"]

    if ctx.kubeconfig is not None:
        command.extend(["--kubeconfig", ctx.kubeconfig])

    command.extend(args)
    return command


def helm(kubectl_ctx: KubectlContext, /, *args):
    command = _get_command(kubectl_ctx, *args)
    print_command(command)
    result = subprocess.run(command)
    handle_error(result)


def helm_install(
    kubectl_ctx: KubectlContext,
    namespace: str,
    repository: str | None = None,
    tag: str | None = None,
    values: dict[str, str] | None = None,
):
    repo = repository if repository else "localhost/pod-graceful-drain"
    tag = tag if tag else "latest"

    set_values_args = []
    if values:
        for key, value in values.items():
            set_values_args.append(f"--set={key}={value}")

    proj_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../..")
    helm(
        kubectl_ctx,
        "upgrade",
        "--install",
        "pod-graceful-drain",
        os.path.join(proj_dir, "charts/pod-graceful-drain"),
        "--create-namespace",
        f"--namespace={namespace}",
        f"--set=image.repository={repo}",
        f"--set=image.tag={tag}",
        f"--set=logLevel=info\\,pod_graceful_drain=trace\\,pod_graceful_drain::reflector=info",
        *set_values_args,
        "--wait=true",
        "--timeout=1m",
    )
