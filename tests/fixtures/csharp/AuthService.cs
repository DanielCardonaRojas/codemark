using System;
using System.Collections.Generic;

namespace CodemarkExample.Auth
{
    // Target: bookmark a record
    public record Claims(string Subject, DateTime Expiry, List<string> Roles);

    // Target: bookmark an enum
    public enum AuthError
    {
        InvalidToken,
        Expired,
        InsufficientPermissions
    }

    // Target: bookmark an interface
    public interface IAuthProvider
    {
        Claims ValidateToken(string token);
        string RefreshToken(string token);
    }

    // Target: bookmark a class
    public class AuthService : IAuthProvider
    {
        private readonly string _secret;
        private readonly string _issuer;
        private readonly Dictionary<string, Claims> _tokenCache = new();

        public AuthService(string secret, string issuer = "codemark")
        {
            _secret = secret;
            _issuer = issuer;
        }

        // Target: bookmark a method
        public Claims ValidateToken(string token)
        {
            if (_tokenCache.TryGetValue(token, out var cached))
            {
                if (cached.Expiry <= DateTime.UtcNow)
                {
                    _tokenCache.Remove(token);
                    throw new InvalidOperationException("Token expired");
                }
                return cached;
            }

            var claims = Decode(token);
            _tokenCache[token] = claims;
            return claims;
        }

        public string RefreshToken(string token)
        {
            var claims = Decode(token);
            return Encode(claims);
        }

        // Target: bookmark a private method
        private Claims Decode(string token)
        {
            if (string.IsNullOrEmpty(token))
            {
                throw new ArgumentException("Invalid token");
            }
            return new Claims("user-1", DateTime.UtcNow.AddHours(1), new List<string> { "admin" });
        }

        private string Encode(Claims claims)
        {
            return $"{_issuer}:{claims.Subject}:{_secret}";
        }

        // Target: bookmark a method with complex params
        public void CheckPermission(Claims claims, string required, string resource)
        {
            if (!claims.Roles.Contains(required))
            {
                throw new UnauthorizedAccessException(
                    $"Insufficient permissions: {required} on {resource}");
            }
        }

        // Target: bookmark a static factory
        public static AuthService CreateDefault()
        {
            return new AuthService("default-secret");
        }
    }
}
