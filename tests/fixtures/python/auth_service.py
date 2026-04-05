"""Authentication service module."""

from dataclasses import dataclass
from datetime import datetime, timedelta
from enum import Enum
from typing import Dict, List, Optional


# Target: bookmark an enum
class AuthError(Enum):
    INVALID_TOKEN = "invalid_token"
    EXPIRED = "expired"
    INSUFFICIENT_PERMISSIONS = "insufficient_permissions"


# Target: bookmark a dataclass
@dataclass
class Claims:
    subject: str
    expiry: datetime
    roles: List[str]


# Target: bookmark a class
class AuthService:
    """Handles authentication and token validation."""

    def __init__(self, secret: str, issuer: str = "codemark"):
        self.secret = secret
        self.issuer = issuer
        self._token_cache: Dict[str, Claims] = {}

    # Target: bookmark a method
    def validate_token(self, token: str) -> Claims:
        cached = self._token_cache.get(token)
        if cached:
            if cached.expiry <= datetime.now():
                del self._token_cache[token]
                raise ValueError(AuthError.EXPIRED.value)
            return cached

        claims = self._decode(token)
        if claims.expiry <= datetime.now():
            raise ValueError(AuthError.EXPIRED.value)

        self._token_cache[token] = claims
        return claims

    # Target: bookmark a private method
    def _decode(self, token: str) -> Claims:
        if not token:
            raise ValueError(AuthError.INVALID_TOKEN.value)
        return Claims(
            subject="user-1",
            expiry=datetime.now() + timedelta(hours=1),
            roles=["admin"],
        )

    # Target: bookmark a method with complex params
    def check_permission(
        self, claims: Claims, required: str, resource: str
    ) -> None:
        if required not in claims.roles:
            raise PermissionError(
                f"{AuthError.INSUFFICIENT_PERMISSIONS.value}: {required} on {resource}"
            )

    # Target: bookmark a decorated method
    @staticmethod
    def create_default() -> "AuthService":
        return AuthService(secret="default-secret")


# Target: bookmark a top-level function
def create_default_auth_service() -> AuthService:
    return AuthService(secret="default-secret")


# Target: bookmark a decorated function
def require_auth(func):
    """Decorator that validates auth before calling the wrapped function."""

    def wrapper(token: str, *args, **kwargs):
        service = create_default_auth_service()
        claims = service.validate_token(token)
        return func(claims, *args, **kwargs)

    return wrapper


# Target: bookmark a function with decorator
@require_auth
def get_user_profile(claims: Claims) -> dict:
    return {"subject": claims.subject, "roles": claims.roles}
