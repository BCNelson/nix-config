const assert = require('node:assert/strict');
const update = require('../nixos/romeo/services/peertube-oidc-role-sync.cjs');

assert.equal(update({ fieldName: 'role', currentValue: 2, newValue: 0 }), 0);
assert.equal(update({ fieldName: 'role', currentValue: 0, newValue: 2 }), 2);
for (const newValue of [undefined, null, '0', 1, -1, NaN]) {
  assert.equal(update({ fieldName: 'role', currentValue: 0, newValue }), 2);
}
for (const fieldName of ['adminFlags', 'videoQuota', 'videoQuotaDaily', 'displayName']) {
  assert.equal(update({ fieldName, currentValue: 'unchanged', newValue: 'from-oidc' }), 'unchanged');
}
console.log('Existing-account role promotion/demotion and unrelated-field preservation pass.');
