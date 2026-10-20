#!/usr/bin/env python3
"""Generate secrets locally, never print them or overwrite an existing installation."""
import json, os, pathlib, secrets, tempfile
root=pathlib.Path(__file__).resolve().parents[1]
directory=root/'.secrets'
directory.mkdir(mode=0o700,exist_ok=True)
os.chmod(directory,0o700)
for name in ['pg_bootstrap_password','pg_migrator_password','pg_api_password','pg_auth_password','oidc_bootstrap_password']:
    path=directory/name
    if not path.exists():
        fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o400)
        with os.fdopen(fd,'w') as f:
            f.write(secrets.token_hex(32)+'\n')
            f.flush();os.fsync(f.fileno())
        # Compose bind-backed secrets keep host ownership. The private parent directory is 0700;
        # only explicitly mounted files are readable by the service's unprivileged container UID.
        path.chmod(0o444)
realm=json.loads((root/'docker/realm-lightcraft.json').read_text())
realm['sslRequired']='none'  # Only the loopback development issuer.
with tempfile.NamedTemporaryFile('w', dir=directory, prefix='realm-', suffix='.tmp', delete=False) as f:
    f.write(json.dumps(realm,indent=2)+'\n'); f.flush(); os.fsync(f.fileno())
    realm_temp=pathlib.Path(f.name)
realm_temp.chmod(0o444)
os.replace(realm_temp,directory/'lightcraft-dev-realm.json')
if not (root/'.env').exists():
    fd=os.open(root/'.env',os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(fd,'w') as f:
        f.write('LC_AUTH_PUBLIC_URL=http://127.0.0.1:18881\nLC_OIDC_ISSUER=http://127.0.0.1:18881/realms/lightcraft\nLC_ALLOW_HTTP_OIDC=true\nLC_STORAGE_QUOTA_BYTES=134217728\nLC_MIN_FREE_BYTES=67108864\nLC_MAX_UPLOAD_BYTES=33554432\n')
# Persist file names as well as secret contents before a database can consume them.
for folder in (directory,root):
    descriptor=os.open(folder,os.O_RDONLY|getattr(os,'O_DIRECTORY',0))
    try:os.fsync(descriptor)
    finally:os.close(descriptor)
print('Local configuration initialized; existing secrets retained. No secret values were printed.')
