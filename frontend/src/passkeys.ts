type JsonDescriptor = Omit<PublicKeyCredentialDescriptor, 'id'> & { id: string };
export interface RegistrationOptions {
  publicKey: Omit<PublicKeyCredentialCreationOptions, 'challenge' | 'user' | 'excludeCredentials'> & {
    challenge: string;
    user: Omit<PublicKeyCredentialUserEntity, 'id'> & { id: string };
    excludeCredentials?: JsonDescriptor[];
  };
}
export interface AuthenticationOptions {
  publicKey: Omit<PublicKeyCredentialRequestOptions, 'challenge' | 'allowCredentials'> & { challenge: string; allowCredentials?: JsonDescriptor[] };
}
const decode = (value: string) => Uint8Array.from(atob(value.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
const encode = (value: ArrayBuffer) => btoa(String.fromCharCode(...new Uint8Array(value))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
function serialize(credential: PublicKeyCredential) {
  const original = credential.response;
  const response: { clientDataJSON: string; attestationObject?: string; authenticatorData?: string; signature?: string; userHandle?: string; transports?: string[] } = { clientDataJSON: encode(original.clientDataJSON) };
  if (original instanceof AuthenticatorAttestationResponse) {
    response.attestationObject = encode(original.attestationObject);
    if (typeof original.getTransports === 'function') response.transports = original.getTransports();
  } else if (original instanceof AuthenticatorAssertionResponse) {
    response.authenticatorData = encode(original.authenticatorData);
    response.signature = encode(original.signature);
    if (original.userHandle) response.userHandle = encode(original.userHandle);
  }
  const extensions = credential.getClientExtensionResults();
  return { id: credential.id, rawId: encode(credential.rawId), type: credential.type, response, extensions, clientExtensionResults: extensions };
}
export async function createPasskey(options: RegistrationOptions) {
  if (!window.PublicKeyCredential) throw new Error('Trình duyệt chưa hỗ trợ passkey. Cần HTTPS hoặc localhost.');
  const source = options.publicKey;
  const publicKey: PublicKeyCredentialCreationOptions = { ...source, challenge: decode(source.challenge), user: { ...source.user, id: decode(source.user.id) }, excludeCredentials: source.excludeCredentials?.map(c => ({ ...c, id: decode(c.id) })) };
  const credential = await navigator.credentials.create({ publicKey });
  if (!(credential instanceof PublicKeyCredential)) throw new Error('Đã hủy đăng ký passkey');
  return serialize(credential);
}
export async function getPasskey(options: AuthenticationOptions) {
  if (!window.PublicKeyCredential) throw new Error('Passkey cần HTTPS hoặc localhost và trình duyệt hỗ trợ.');
  const source = options.publicKey;
  const publicKey: PublicKeyCredentialRequestOptions = { ...source, challenge: decode(source.challenge), allowCredentials: source.allowCredentials?.map(c => ({ ...c, id: decode(c.id) })) };
  const credential = await navigator.credentials.get({ publicKey });
  if (!(credential instanceof PublicKeyCredential)) throw new Error('Đã hủy đăng nhập');
  return serialize(credential);
}
