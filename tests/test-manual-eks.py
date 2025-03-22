from pathlib import Path
from tempfile import TemporaryDirectory
import os

import click

from utils.aws import EcrContext, get_test_aws_profile
from utils.eksctl import EksctlContext
from utils.docker import DockerContext, docker
from utils.helm import helm_install, helm
from utils.kubectl import KubectlContext, kubectl_stdin


@click.group()
def cli():
    pass


@cli.command()
def setup_cluster():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        eks_ctx.create_cluster()

        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        ecr_ctx.create_repository()


@cli.command()
def setup_addons():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)

        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        kubectl_ctx = KubectlContext(eks_ctx.get_kubeconfig(), namespace=None)
        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        docker_ctx = DockerContext(
            tmp_path, ecr_ctx.login_password, ecr_ctx.proxy_endpoint
        )

        install_alb_controller(eks_ctx, kubectl_ctx)
        install_pgd(docker_ctx, ecr_ctx, kubectl_ctx)


def install_alb_controller(eks_ctx, kubectl_ctx):
    helm(
        kubectl_ctx,
        "upgrade",
        "--install",
        "aws-load-balancer-controller",
        "aws-load-balancer-controller",
        "--repo=https://aws.github.io/eks-charts",
        "--namespace=kube-system",
        f"--set=clusterName={eks_ctx.cluster_name}",
        "--set=serviceAccount.create=false",
        "--set=serviceAccount.name=aws-load-balancer-controller",
        "--set=podDisruptionBudget.minAvailable=1",
        "--wait=true",
    )


def install_pgd(docker_ctx, ecr_ctx, kubectl_ctx):
    docker(
        docker_ctx,
        "image",
        "push",
        "--authfile",
        docker_ctx.authfile,
        "localhost/pod-graceful-drain:latest",
        f"{ecr_ctx.repository_uri}:latest",
    )
    helm_install(
        kubectl_ctx,
        "pod-graceful-drain",
        ecr_ctx.repository_uri,
        values={"image.pullPolicy": "Always"},
    )


@cli.command()
def write_kubeconfig():
    aws_profile = get_test_aws_profile()

    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        eks_ctx.write_kubeconfig()


@cli.command()
def update_image():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")

        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        docker_ctx = DockerContext(
            tmp_path, ecr_ctx.login_password, ecr_ctx.proxy_endpoint
        )
        docker(
            docker_ctx,
            "image",
            "push",
            "--authfile",
            docker_ctx.authfile,
            "localhost/pod-graceful-drain:latest",
            f"{ecr_ctx.repository_uri}:latest",
        )

        kubectl_ctx = KubectlContext(eks_ctx.get_kubeconfig(), "default")
        helm_install(
            kubectl_ctx,
            "pod-graceful-drain",
            ecr_ctx.repository_uri,
            values={"image.pullPolicy": "Always"},
        )


@cli.command()
def cleanup():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        ecr_ctx.cleanup_repository()
        eks_ctx.cleanup_cluster()


if __name__ == "__main__":
    cli()
