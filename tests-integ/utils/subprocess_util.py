from subprocess import CompletedProcess


def print_command(args):
    print(f"> {" ".join(args)}")


def to_bytes(s):
    if isinstance(s, bytes):
        return s

    return bytes(s, "utf-8")


def handle_error(result: CompletedProcess):
    if result.returncode == 0:
        return

    raise Exception(f"Command failed with exit code '{result.returncode}'")
