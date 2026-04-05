// Target: bookmark an interface
export interface Claims {
  subject: string;
  expiry: Date;
  roles: string[];
}

// Target: bookmark an enum
export enum AuthError {
  InvalidToken = "INVALID_TOKEN",
  Expired = "EXPIRED",
  InsufficientPermissions = "INSUFFICIENT_PERMISSIONS",
}

// Target: bookmark a class declaration
export class AuthService {
  private secret: string;
  private issuer: string;
  private tokenCache: Map<string, Claims> = new Map();

  constructor(secret: string, issuer: string = "codemark") {
    this.secret = secret;
    this.issuer = issuer;
  }

  // Target: bookmark a method
  async validateToken(token: string): Promise<Claims> {
    const cached = this.tokenCache.get(token);
    if (cached) {
      if (cached.expiry <= new Date()) {
        this.tokenCache.delete(token);
        throw new Error(AuthError.Expired);
      }
      return cached;
    }

    const claims = this.decode(token);
    if (claims.expiry <= new Date()) {
      throw new Error(AuthError.Expired);
    }

    this.tokenCache.set(token, claims);
    return claims;
  }

  // Target: bookmark a private method
  private decode(token: string): Claims {
    if (!token) {
      throw new Error(AuthError.InvalidToken);
    }
    return {
      subject: "user-1",
      expiry: new Date(Date.now() + 3600000),
      roles: ["admin"],
    };
  }

  // Target: bookmark a method with complex params
  checkPermission(claims: Claims, required: string, resource: string): void {
    if (!claims.roles.includes(required)) {
      throw new Error(`${AuthError.InsufficientPermissions}: ${required} on ${resource}`);
    }
  }
}

// Target: bookmark an arrow function
export const createDefaultAuthService = (): AuthService => {
  return new AuthService("default-secret");
};

// Target: bookmark a regular function
export function validateAndCheck(
  service: AuthService,
  token: string,
  requiredRole: string
): Promise<Claims> {
  return service.validateToken(token).then((claims) => {
    service.checkPermission(claims, requiredRole, "*");
    return claims;
  });
}

// Target: bookmark a type alias
export type AuthProvider = {
  validateToken(token: string): Promise<Claims>;
  refreshToken(token: string): Promise<string>;
};
