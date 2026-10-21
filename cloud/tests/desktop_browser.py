#!/usr/bin/env python3
"""Isolated desktop E2E browser driver. Credentials stay in ignored local files.

prepare creates a temporary account; login consumes the desktop's authorization URL
on stdin and follows the real Keycloak form back to its loopback PKCE listener.
It neither mints tokens nor changes the desktop's authentication checks.
"""
import json, os, pathlib, secrets, sys, urllib.parse, urllib.request, http.cookiejar, uuid
from e2e import AUTH, ROOT, NoRedirect, LoopbackCookiePolicy, LoginForm, request, parsed, form

STATE=ROOT/'.secrets/desktop-e2e.json'
if urllib.parse.urlparse(AUTH).hostname not in ('127.0.0.1','localhost'):
    raise SystemExit('Desktop tests require an isolated loopback Keycloak.')

def prepare():
    bootstrap=(ROOT/'.secrets/oidc_bootstrap_password').read_text().strip()
    status,_,admin=form(AUTH+'/realms/master/protocol/openid-connect/token',{'client_id':'admin-cli','grant_type':'password','username':'lc-bootstrap','password':bootstrap})
    if status!=200:raise RuntimeError('Test administrator authentication failed.')
    username='lc-desktop-'+uuid.uuid4().hex[:12]
    password=secrets.token_urlsafe(32)
    status,_,_=parsed('POST',AUTH+'/admin/realms/lightcraft/users',{'username':username,'email':username+'@example.invalid','emailVerified':True,'firstName':'Desktop','lastName':'Test','enabled':True,'credentials':[{'type':'password','value':password,'temporary':False}]},admin['access_token'])
    if status!=201:raise RuntimeError('Test account creation failed.')
    descriptor=os.open(STATE,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
    with os.fdopen(descriptor,'w') as output:json.dump({'username':username,'password':password},output)
    print('Prepared an isolated desktop test account; no credentials printed.')

def login():
    url=sys.stdin.readline(16384).strip()
    parsed_url=urllib.parse.urlparse(url)
    if parsed_url.netloc!=urllib.parse.urlparse(AUTH).netloc or parsed_url.path!='/realms/lightcraft/protocol/openid-connect/auth':
        raise RuntimeError('Unexpected authorization destination.')
    account=json.loads(STATE.read_text())
    opener=urllib.request.build_opener(NoRedirect,urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar(policy=LoopbackCookiePolicy())))
    status,_,body=request('GET',url,opener=opener)
    if status!=200:raise RuntimeError('Authorization page unavailable.')
    page=LoginForm();page.feed(body.decode())
    if not page.action or urllib.parse.urlparse(page.action).netloc!=parsed_url.netloc:raise RuntimeError('Unexpected login form.')
    status,headers,_=request('POST',page.action,urllib.parse.urlencode(page.fields|account|{'credentialId':''}).encode(),headers={'Content-Type':'application/x-www-form-urlencoded'},opener=opener)
    callback=headers.get('Location','')
    endpoint=urllib.parse.urlparse(callback)
    if status!=302 or endpoint.scheme!='http' or endpoint.netloc!='127.0.0.1:47861' or endpoint.path!='/callback':
        raise RuntimeError('Expected the desktop loopback callback.')
    if request('GET',callback,opener=opener)[0]!=200:raise RuntimeError('Desktop rejected the authorization response.')

if __name__=='__main__':
    if sys.argv[1:] == ['prepare']:prepare()
    elif sys.argv[1:] == ['login']:login()
    else:raise SystemExit('Use prepare or login.')
