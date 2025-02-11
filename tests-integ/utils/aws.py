import base64
import os

import boto3


def get_test_aws_profile():
    # We use different environment variable to prevent accidentally test on other environment
    aws_profile = os.environ["PGD_TEST_AWS_PROFILE"]
    if not aws_profile:
        raise Exception("'PGD_TEST_AWS_PROFILE' environment variable is not set")

    # to prevent accidental leak
    os.environ["AWS_PROFILE"] = aws_profile
    return aws_profile


class EcrContext:
    def __init__(self, aws_profile: str, repository_name: str):
        self.repository_name = repository_name

        session = boto3.Session(profile_name=aws_profile)
        self.client = session.client("ecr")

        self.login_password, self.proxy_endpoint = (
            self._fetch_login_password_and_proxy()
        )
        self.repository_uri = self.proxy_endpoint + "/" + self.repository_name

    def _fetch_login_password_and_proxy(self):
        result = self.client.get_authorization_token()
        data = result["authorizationData"][0]
        base64_token = data["authorizationToken"]
        _, login_password = str(base64.b64decode(base64_token), "utf-8").split(":")
        proxy_endpoint = data["proxyEndpoint"].replace("https://", "")
        return login_password, proxy_endpoint

    def create_repository(self):
        self.client.create_repository(
            repositoryName=self.repository_name,
            tags=[
                {"Key": "test-pgd/cluster-name", "Value": self.repository_name},
            ],
        )

    def cleanup_repository(self):
        try:
            self.client.delete_repository(
                repositoryName=self.repository_name, force=True
            )
        except:
            pass

    def __enter__(self):
        self.create_repository()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.cleanup_repository()
