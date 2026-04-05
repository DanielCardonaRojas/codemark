package com.example.auth;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

// Target: bookmark an interface
interface AuthProvider {
    Claims validateToken(String token) throws AuthException;
    String refreshToken(String token) throws AuthException;
}

// Target: bookmark a record/class
class Claims {
    final String subject;
    final long expiry;
    final List<String> roles;

    Claims(String subject, long expiry, List<String> roles) {
        this.subject = subject;
        this.expiry = expiry;
        this.roles = roles;
    }
}

// Target: bookmark an enum
enum AuthError {
    INVALID_TOKEN,
    EXPIRED,
    INSUFFICIENT_PERMISSIONS
}

// Target: bookmark an exception class
class AuthException extends Exception {
    private final AuthError error;

    AuthException(AuthError error, String message) {
        super(message);
        this.error = error;
    }

    AuthError getError() {
        return error;
    }
}

// Target: bookmark a class
public class AuthService implements AuthProvider {
    private final String secret;
    private final String issuer;
    private final Map<String, Claims> tokenCache = new HashMap<>();

    public AuthService(String secret, String issuer) {
        this.secret = secret;
        this.issuer = issuer;
    }

    // Target: bookmark a method
    @Override
    public Claims validateToken(String token) throws AuthException {
        Claims cached = tokenCache.get(token);
        if (cached != null) {
            if (cached.expiry <= System.currentTimeMillis()) {
                tokenCache.remove(token);
                throw new AuthException(AuthError.EXPIRED, "Token expired");
            }
            return cached;
        }

        Claims claims = decode(token);
        tokenCache.put(token, claims);
        return claims;
    }

    @Override
    public String refreshToken(String token) throws AuthException {
        Claims claims = decode(token);
        return encode(claims);
    }

    // Target: bookmark a private method
    private Claims decode(String token) throws AuthException {
        if (token == null || token.isEmpty()) {
            throw new AuthException(AuthError.INVALID_TOKEN, "Invalid token");
        }
        return new Claims("user-1", System.currentTimeMillis() + 3600000, List.of("admin"));
    }

    private String encode(Claims claims) {
        return issuer + ":" + claims.subject + ":" + secret;
    }

    // Target: bookmark a method with complex params
    public void checkPermission(Claims claims, String required, String resource)
            throws AuthException {
        if (!claims.roles.contains(required)) {
            throw new AuthException(
                AuthError.INSUFFICIENT_PERMISSIONS,
                required + " on " + resource
            );
        }
    }

    // Target: bookmark a static factory
    public static AuthService createDefault() {
        return new AuthService("default-secret", "codemark");
    }
}
