import os
from pathlib import Path
from tempfile import TemporaryDirectory

import click

from utils.aws import EcrContext, get_test_aws_profile
from utils.eksctl import EksctlContext
from utils.docker import DockerContext, docker
from utils.helm import helm_install


@click.group()
def cli():
    pass


@cli.command()
def setup():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        eks_ctx.create_cluster()

        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        ecr_ctx.create_repository()

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

        kubectl_ctx = eks_ctx.get_kubeconfig()
        helm_install(kubectl_ctx, ecr_ctx.repository_uri)


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
