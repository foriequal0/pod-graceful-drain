import subprocess
from pathlib import Path

from .subprocess_util import handle_error, print_command, to_bytes


class DockerContext:
    def __init__(self, tmp_path: Path, login_password: str, proxy_endpoint: str):
        self.login_password = login_password
        self.proxy_endpoint = proxy_endpoint
        self.authfile = str(tmp_path / "docker.authfile")
        self.login()

    def login(self):
        docker_stdin(
            self,
            "login",
            "--authfile",
            self.authfile,
            "--username",
            "AWS",
            "--password-stdin",
            self.proxy_endpoint,
            stdin=self.login_password,
        )


def _get_command(_ctx: DockerContext, /, *args):
    command = ["docker"]
    command.extend(args)
    return command


def docker(ctx: DockerContext, /, *args):
    command = _get_command(ctx, *args)
    print_command(command)
    result = subprocess.run(command)
    handle_error(result)


def docker_stdin(ctx: DockerContext, /, *args, stdin):
    command = _get_command(ctx, *args)
    print_command([*command, f"<<<EOF\n{stdin}\nEOF"])
    result = subprocess.run(command, input=to_bytes(stdin))
    handle_error(result)


def docker_tag(ctx: DockerContext, /, image, tag):
    command = _get_command(ctx, "tag", image, tag)
    print_command(command)
    result = subprocess.run(command)
    handle_error(result)
