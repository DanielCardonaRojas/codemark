// Target: bookmark an enum
enum AuthError {
  invalidToken,
  expired,
  insufficientPermissions,
}

// Target: bookmark a class
class Claims {
  final String subject;
  final DateTime expiry;
  final List<String> roles;

  Claims({required this.subject, required this.expiry, required this.roles});
}

// Target: bookmark an abstract class (interface)
abstract class AuthProvider {
  Future<Claims> validateToken(String token);
  Future<String> refreshToken(String token);
}

// Target: bookmark a class with methods
class AuthService implements AuthProvider {
  final String _secret;
  final String _issuer;
  final Map<String, Claims> _tokenCache = {};

  AuthService(this._secret, {String issuer = 'codemark'}) : _issuer = issuer;

  // Target: bookmark a method
  @override
  Future<Claims> validateToken(String token) async {
    final cached = _tokenCache[token];
    if (cached != null) {
      if (cached.expiry.isBefore(DateTime.now())) {
        _tokenCache.remove(token);
        throw Exception(AuthError.expired.name);
      }
      return cached;
    }

    final claims = _decode(token);
    _tokenCache[token] = claims;
    return claims;
  }

  @override
  Future<String> refreshToken(String token) async {
    final claims = _decode(token);
    return _encode(claims);
  }

  // Target: bookmark a private method
  Claims _decode(String token) {
    if (token.isEmpty) {
      throw ArgumentError(AuthError.invalidToken.name);
    }
    return Claims(
      subject: 'user-1',
      expiry: DateTime.now().add(const Duration(hours: 1)),
      roles: ['admin'],
    );
  }

  String _encode(Claims claims) {
    return '$_issuer:${claims.subject}:$_secret';
  }

  // Target: bookmark a method with complex params
  void checkPermission(Claims claims, String required, String resource) {
    if (!claims.roles.contains(required)) {
      throw Exception('${AuthError.insufficientPermissions.name}: $required on $resource');
    }
  }

  // Target: bookmark a static factory
  static AuthService createDefault() {
    return AuthService('default-secret');
  }
}

// Target: bookmark a top-level function
AuthService createDefaultAuthService() {
  return AuthService.createDefault();
}
