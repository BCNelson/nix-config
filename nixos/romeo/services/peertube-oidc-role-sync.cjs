// PeerTube invokes this hook for existing external-auth accounts. Keep other
// account settings untouched; only the signed OIDC role can grant admin access.
module.exports = ({ fieldName, currentValue, newValue }) => {
  if (fieldName !== 'role') return currentValue;
  return newValue === 0 ? 0 : 2;
};
