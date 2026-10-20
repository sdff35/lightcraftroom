#!/usr/bin/env python3
"""Real PostgreSQL + Keycloak + HTTP tests. Loopback Codex/development stack only.
No mocked authentication, credential values, committed fixture images or public DB port.
"""
import base64, hashlib, html, http.cookiejar, json, os, pathlib, re, secrets, socket, struct, subprocess, time, unittest, urllib.error, urllib.parse, urllib.request, uuid
from html.parser import HTMLParser
ROOT=pathlib.Path(__file__).resolve().parents[1]
API=os.environ.get('LC_TEST_API_URL','http://127.0.0.1:18880')
AUTH=os.environ.get('LC_TEST_AUTH_URL','http://127.0.0.1:18881')
for endpoint in (API,AUTH):
    if urllib.parse.urlparse(endpoint).hostname not in ('127.0.0.1','localhost'):
        raise SystemExit('Tests require an isolated loopback stack, never a production server.')
PROJECT=os.environ.get('LC_COMPOSE_PROJECT','lightcraft-cloud-codex')
DOCKER=os.environ.get('LC_TEST_DOCKER','docker')
COMPOSE=[DOCKER,'--host=unix:///var/run/docker.sock','compose','--project-directory',str(ROOT),'-p',PROJECT,'-f',str(ROOT/'compose.yaml'),'-f',str(ROOT/'compose.local.yaml')]
for extra in os.environ.get('LC_TEST_COMPOSE_EXTRA','').split(os.pathsep):
    if extra: COMPOSE.extend(['-f',extra])

class LoopbackCookiePolicy(http.cookiejar.DefaultCookiePolicy):
    # Browsers treat HTTP loopback as a trustworthy local context; Keycloak 26.8 sets
    # Secure cookies there. Match that browser exception only for this isolated test.
    def return_ok_secure(self,cookie,request):
        url=urllib.parse.urlparse(request.full_url)
        if url.scheme=='http' and url.hostname in ('127.0.0.1','localhost'):return True
        return super().return_ok_secure(cookie,request)

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args): return None

def request(method,url,data=None,token=None,headers=None,opener=None):
    headers=dict(headers or {})
    if isinstance(data,(dict,list)):
        data=json.dumps(data).encode();headers['Content-Type']='application/json'
    if token: headers['Authorization']='Bearer '+token
    req=urllib.request.Request(url,data=data,method=method,headers=headers)
    try:
        with (opener.open(req,timeout=30) if opener else urllib.request.urlopen(req,timeout=30)) as r:
            return r.status,r.headers,r.read()
    except urllib.error.HTTPError as e: return e.code,e.headers,e.read()

def parsed(method,url,data=None,token=None,headers=None,opener=None):
    status,h,body=request(method,url,data,token,headers,opener)
    try: value=json.loads(body)
    except json.JSONDecodeError: value=None
    return status,h,value

def form(url,data): return parsed('POST',url,urllib.parse.urlencode(data).encode(),headers={'Content-Type':'application/x-www-form-urlencoded'})
def docker(*args,check=True):
    environment=dict(os.environ)
    for name in ('DOCKER_HOST','DOCKER_CONTEXT','DOCKER_TLS','DOCKER_TLS_VERIFY','DOCKER_CERT_PATH'):
        environment.pop(name,None)
    return subprocess.run(COMPOSE+list(args),capture_output=True,text=True,timeout=600,check=check,env=environment)
def sql(statement,runtime=False,check=True):
    if runtime:
        command='export PGPASSWORD="$(cat /run/secrets/pg_api_password)"; exec psql -h 127.0.0.1 -U lightcraft_api -d lightcraft -At -v ON_ERROR_STOP=1 -c "$1"'
    else:
        command='exec psql -U postgres -d lightcraft -At -v ON_ERROR_STOP=1 -c "$1"'
    return docker('exec','-T','lc-db','sh','-ec',command,'sh',statement,check=check)
def jpeg():
    # Original procedural 8x8 grayscale JPEG, constant DCT coefficients. No third-party image.
    def segment(marker,payload): return b'\xff'+bytes([marker])+struct.pack('>H',len(payload)+2)+payload
    quant=segment(0xdb,b'\x00'+bytes([1])*64)
    frame=segment(0xc0,b'\x08'+struct.pack('>HH',8,8)+b'\x01\x01\x11\x00')
    counts=b'\x01'+bytes(15)
    huffman=segment(0xc4,b'\x00'+counts+b'\x00'+b'\x10'+counts+b'\x00')
    scan=segment(0xda,b'\x01\x01\x00\x00\x3f\x00')
    return b'\xff\xd8'+quant+frame+huffman+scan+b'\x3f\xff\xd9'
def dng():
    # Original small TIFF/DNG envelope with one 2x2 16-bit grayscale strip.
    entries=[(256,4,1,2),(257,4,1,2),(258,3,1,16),(259,3,1,1),(262,3,1,1),(273,4,1,8+2+10*12+4),(277,3,1,1),(278,4,1,2),(279,4,1,8),(50706,1,4,0x00000401)]
    return b'II\x2a\0'+struct.pack('<I',8)+struct.pack('<H',len(entries))+b''.join(struct.pack('<HHII',*e) for e in entries)+struct.pack('<I',0)+struct.pack('<HHHH',100,200,300,400)

class LoginForm(HTMLParser):
    def __init__(self):super().__init__();self.action=None;self.fields={};self.in_login=False
    def handle_starttag(self,tag,attrs):
        a=dict(attrs)
        if tag=='form':
            self.in_login=a.get('id')=='kc-form-login'
            if self.in_login:self.action=a.get('action')
        if self.in_login and tag=='input' and a.get('type')=='hidden' and a.get('name'):self.fields[a['name']]=a.get('value','')
    def handle_endtag(self,tag):
        if tag=='form':self.in_login=False

class CloudTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if parsed('GET',API+'/health/ready')[0]!=200:raise RuntimeError('Start the isolated Compose stack and wait for readiness before running tests.')
        bootstrap=(ROOT/'.secrets/oidc_bootstrap_password').read_text().strip()
        status,_,admin=form(AUTH+'/realms/master/protocol/openid-connect/token',{'client_id':'admin-cli','grant_type':'password','username':'lc-bootstrap','password':bootstrap})
        if status!=200:raise RuntimeError('Local Keycloak bootstrap authentication failed (secret omitted).')
        cls.admin=admin['access_token'];cls.users=[];cls.created_clients=[]
        for label in ('a','b'):
            name='lc-stage3-'+label+'-'+uuid.uuid4().hex[:12];password=secrets.token_urlsafe(32)
            status,_,_=parsed('POST',AUTH+'/admin/realms/lightcraft/users',{'username':name,'email':name+'@example.invalid','emailVerified':True,'firstName':'Test','lastName':label,'enabled':True,'credentials':[{'type':'password','value':password,'temporary':False}]},cls.admin)
            if status!=201:raise RuntimeError('Creating local test account failed.')
            status,_,users=parsed('GET',AUTH+'/admin/realms/lightcraft/users?'+urllib.parse.urlencode({'username':name,'exact':'true'}),token=cls.admin)
            cls.users.append({'name':name,'password':password,'id':users[0]['id']})
        cls.tokens=[cls.login(u) for u in cls.users]
        for user,tokens in zip(cls.users,cls.tokens):
            payload=tokens['access_token'].split('.')[1]
            claims=json.loads(base64.urlsafe_b64decode(payload+'='*(-len(payload)%4)))
            if claims.get('sub')!=user['id']:raise RuntimeError('The configured stable OIDC subject mapper is missing or incorrect.')
    @classmethod
    def login(cls,user,client='lightcraft-desktop'):
        verifier=secrets.token_urlsafe(48);challenge=base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b'=').decode()
        redirect='http://127.0.0.1:47861/callback';state=secrets.token_urlsafe(24)
        opener=urllib.request.build_opener(NoRedirect,urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar(policy=LoopbackCookiePolicy())))
        query=urllib.parse.urlencode({'client_id':client,'response_type':'code','redirect_uri':redirect,'scope':'openid','state':state,'code_challenge':challenge,'code_challenge_method':'S256'})
        status,_,body=request('GET',AUTH+'/realms/lightcraft/protocol/openid-connect/auth?'+query,opener=opener)
        if status!=200:raise RuntimeError('OIDC authorization page failed.')
        page=LoginForm();page.feed(body.decode())
        if not page.action or urllib.parse.urlparse(page.action).netloc!=urllib.parse.urlparse(AUTH).netloc:raise RuntimeError('Unexpected Keycloak login form.')
        fields=page.fields|{'username':user['name'],'password':user['password'],'credentialId':''}
        status,headers,reply=request('POST',html.unescape(page.action),urllib.parse.urlencode(fields).encode(),headers={'Content-Type':'application/x-www-form-urlencoded'},opener=opener)
        location=headers.get('Location','')
        if status!=302 or not location.startswith(redirect+'?'):
            category='invalid credentials' if b'Invalid username' in reply else ('required action' if b'UPDATE_PROFILE' in reply or b'Update Account' in reply else 'unexpected login response')
            feedback=re.findall(r'<(?:span|p)[^>]*(?:kc-feedback-text|instruction|error-message)[^>]*>(.*?)</(?:span|p)>',reply.decode(),flags=re.S)
            if feedback: category+=': '+re.sub(r'<[^>]+>',' ',feedback[0]).strip()[:180]
            raise RuntimeError('Local login failed: '+str(status)+' '+urllib.parse.urlparse(location).path+' ('+category+').')
        callback=urllib.parse.parse_qs(urllib.parse.urlparse(location).query)
        if callback.get('state')!=[state]:raise RuntimeError('OIDC state mismatch.')
        status,_,tokens=form(AUTH+'/realms/lightcraft/protocol/openid-connect/token',{'client_id':client,'grant_type':'authorization_code','code':callback['code'][0],'redirect_uri':redirect,'code_verifier':verifier})
        if status!=200:raise RuntimeError('PKCE code exchange failed.')
        return tokens
    @classmethod
    def tearDownClass(cls):
        # Keep photo ownership records intact for persistence inspection. Only temporary login
        # clients are removed; test users and their non-secret IDs remain in this isolated DB.
        for client in cls.created_clients:
            parsed('DELETE',AUTH+'/admin/realms/lightcraft/clients/'+client,token=cls.admin)
    def token(self,owner=0):
        token=self.tokens[owner]['access_token']
        payload=token.split('.')[1]
        expiration=json.loads(base64.urlsafe_b64decode(payload+'='*(-len(payload)%4)))['exp']
        if expiration < time.time()+30:
            self.tokens[owner]=self.login(self.users[owner])
        return self.tokens[owner]['access_token']
    def create_library(self):
        status,_,library=parsed('POST',API+'/v1/libraries',{'name':'Stage 3 '+uuid.uuid4().hex[:8]},self.token());self.assertEqual(status,201);return library['id']
    def register(self,library,data=None,name='original.jpg',media='image/jpeg',extra=None):
        data=jpeg() if data is None else data
        payload={'operation_id':str(uuid.uuid4()),'original_name':name,'media_type':media,'byte_length':len(data),'sha256':hashlib.sha256(data).hexdigest(),'metadata':{'title':'Untouched original','camera_make':'Synthetic','rating':4,'tags':['test','RAW/JPEG']}}
        payload.update(extra or {})
        status,_,photo=parsed('POST',API+'/v1/libraries/'+library+'/photos',payload,self.token());self.assertIn(status,(200,201));return photo,payload
    def upload(self,photo,data,expected=201):
        status,_,result=parsed('PUT',API+'/v1/photos/'+photo['id']+'/original',data,self.token(),{'Content-Type':photo['media_type']});self.assertEqual(status,expected);return result
    def test_health_authentication_and_scopes(self):
        self.assertEqual(parsed('GET',API+'/health/live')[0],200)
        status,h,error=parsed('GET',API+'/v1/me');self.assertEqual(status,401);self.assertEqual(h['WWW-Authenticate'],'Bearer');self.assertEqual(error['error']['code'],'unauthorized')
        self.assertEqual(parsed('GET',API+'/v1/me',token='not.a.jwt')[0],401)
        token=self.token();parts=token.split('.');parts[2]=('B' if parts[2].startswith('A') else 'A')+parts[2][1:];self.assertEqual(parsed('GET',API+'/v1/me',token='.'.join(parts))[0],401)
        self.assertEqual(parsed('GET',API+'/v1/me',token=self.tokens[0]['id_token'])[0],401)
        for audience,identity,expected in [(True,True,403),(False,True,401),(True,False,401)]:
            client='lc-read-'+uuid.uuid4().hex;config={'clientId':client,'enabled':True,'publicClient':True,'standardFlowEnabled':True,'directAccessGrantsEnabled':False,'redirectUris':['http://127.0.0.1:47861/callback'],'attributes':{'pkce.code.challenge.method':'S256'},'defaultClientScopes':['lightcraft:read']+(['lightcraft-identity'] if identity else [])+(['lightcraft-audience'] if audience else [])}
            status,h,_=parsed('POST',AUTH+'/admin/realms/lightcraft/clients',config,self.admin);self.assertEqual(status,201);self.created_clients.append(h['Location'].rsplit('/',1)[-1])
            token=self.login(self.users[0],client)['access_token']
            self.assertEqual(parsed('POST',API+'/v1/libraries',{'name':'Forbidden'},token)[0],expected)
    def test_oidc_pkce_and_refresh_rotation(self):
        self.assertEqual(parsed('GET',API+'/v1/me',token=self.token())[0],200)
        refresh=self.tokens[0]['refresh_token']
        status,_,tokens=form(AUTH+'/realms/lightcraft/protocol/openid-connect/token',{'client_id':'lightcraft-desktop','grant_type':'refresh_token','refresh_token':refresh});self.assertEqual(status,200)
        self.tokens[0]=tokens
        self.assertEqual(form(AUTH+'/realms/lightcraft/protocol/openid-connect/token',{'client_id':'lightcraft-desktop','grant_type':'refresh_token','refresh_token':refresh})[0],400)
        self.assertEqual(form(AUTH+'/realms/lightcraft/protocol/openid-connect/token',{'client_id':'lightcraft-desktop','grant_type':'password','username':self.users[0]['name'],'password':self.users[0]['password']})[0],400)
    def test_jpeg_and_raw_are_byte_identical_and_metadata_is_persisted(self):
        library=self.create_library()
        for data,name,media in [(jpeg(),'original.jpg','image/jpeg'),(dng(),'original.dng','application/octet-stream')]:
            p,_=self.register(library,data,name,media);self.assertEqual(p['status'],'pending');self.assertEqual(parsed('GET',API+'/v1/photos/'+p['id']+'/original',token=self.token())[0],409)
            ready=self.upload(p,data);self.assertEqual(ready['status'],'ready');self.assertEqual(ready['revision'],2)
            status,h,download=request('GET',API+'/v1/photos/'+p['id']+'/original',token=self.token());self.assertEqual(status,200);self.assertEqual(download,data);self.assertEqual(h['X-Content-Sha256'],hashlib.sha256(data).hexdigest());self.assertEqual(h['X-Content-Type-Options'],'nosniff')
            status,_,metadata=parsed('GET',API+'/v1/photos/'+p['id'],token=self.token());self.assertEqual(status,200);self.assertEqual(metadata['metadata']['rating'],4);self.assertEqual(metadata['metadata']['title'],'Untouched original')
            status,_,verified=parsed('POST',API+'/v1/photos/'+p['id']+'/verify',token=self.token());self.assertEqual(status,200);self.assertTrue(verified['verified'])
    def test_owner_acl_lists_and_objects(self):
        library=self.create_library();p,_=self.register(library);self.upload(p,jpeg())
        for method,path,body in [('GET','/v1/libraries/'+library,None),('GET','/v1/libraries/'+library+'/photos',None),('GET','/v1/photos/'+p['id'],None),('GET','/v1/photos/'+p['id']+'/original',None),('POST','/v1/photos/'+p['id']+'/verify',None),('PUT','/v1/photos/'+p['id']+'/original',jpeg()),('DELETE','/v1/photos/'+p['id']+'/upload',None)]:
            self.assertEqual(request(method,API+path,body,self.token(1),{'Content-Type':'image/jpeg'})[0],404)
        status,_,listed=parsed('GET',API+'/v1/libraries',token=self.token(1));self.assertEqual(status,200);self.assertNotIn(library,[i['id'] for i in listed['items']])
    def test_invalid_checksum_type_and_body_are_retriable(self):
        library=self.create_library();data=jpeg();p,_=self.register(library)
        self.upload(p,bytes(len(data)),expected=422)
        self.assertEqual(sql("SELECT status FROM photos WHERE id='"+str(uuid.UUID(p['id']))+"'").stdout.strip(),'pending')
        self.upload(p,data[:-1],expected=400)
        endpoint=urllib.parse.urlparse(API)
        with socket.create_connection((endpoint.hostname,endpoint.port),timeout=10) as connection:
            headers=('PUT /v1/photos/'+p['id']+'/original HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer '+self.token()+'\r\nContent-Type: image/jpeg\r\nContent-Length: '+str(len(data))+'\r\nConnection: close\r\n\r\n').encode()
            connection.sendall(headers+data[:8]);connection.shutdown(socket.SHUT_WR)
            response=connection.recv(4096)
            self.assertIn(b'400',response.split(b'\r\n')[0])
        self.assertEqual(request('PUT',API+'/v1/photos/'+p['id']+'/original',data,self.token(),{'Content-Type':'text/plain'})[0],415)
        self.upload(p,data)
        self.upload(p,bytes(len(data)),expected=200) # idempotent completed PUT never overwrites bytes
        self.assertEqual(request('GET',API+'/v1/photos/'+p['id']+'/original',token=self.token())[2],data)
        bad=bytes(64);p,_=self.register(library,bad,name='fake.dng',media='application/octet-stream');self.upload(p,bad,expected=415)
        status,_,cancelled=parsed('DELETE',API+'/v1/photos/'+p['id']+'/upload',token=self.token());self.assertEqual(status,200);self.assertEqual(cancelled['status'],'cancelled')
    def test_idempotency_validation_and_quota(self):
        library=self.create_library();p,payload=self.register(library)
        status,_,again=parsed('POST',API+'/v1/libraries/'+library+'/photos',payload,self.token());self.assertEqual(status,200);self.assertEqual(p['id'],again['id'])
        bad=payload|{'original_name':'other.jpg'};self.assertEqual(parsed('POST',API+'/v1/libraries/'+library+'/photos',bad,self.token())[0],409)
        for bad in [payload|{'operation_id':str(uuid.uuid4()),'original_name':'../bad.jpg'},payload|{'operation_id':str(uuid.uuid4()),'byte_length':0},payload|{'operation_id':str(uuid.uuid4()),'metadata':{'rating':6}}]:
            self.assertEqual(parsed('POST',API+'/v1/libraries/'+library+'/photos',bad,self.token())[0],400)
        self.assertEqual(parsed('POST',API+'/v1/libraries',{'name':'x'*70000},self.token())[0],413)
        self.assertEqual(parsed('GET',API+'/v1/libraries?limit=101',token=self.token())[0],400)
        self.assertEqual(parsed('GET',API+'/v1/photos/not-a-uuid',token=self.token())[0],400)
        old=sql('SELECT reserved_bytes FROM storage_budget').stdout.strip()
        sql('UPDATE storage_budget SET reserved_bytes=200000000 WHERE singleton=true')
        try:
            bad=payload|{'operation_id':str(uuid.uuid4())};self.assertEqual(parsed('POST',API+'/v1/libraries/'+library+'/photos',bad,self.token())[0],507)
        finally: sql('UPDATE storage_budget SET reserved_bytes='+str(int(old))+' WHERE singleton=true')
    def test_postgresql_roles_migrations_and_change_order(self):
        self.assertEqual(int(sql('SELECT count(*) FROM _sqlx_migrations WHERE success').stdout.strip()),1)
        self.assertNotEqual(docker('port','lc-db','5432',check=False).returncode,0)
        self.assertNotEqual(sql('CREATE TABLE forbidden(id int)',runtime=True,check=False).returncode,0)
        self.assertNotEqual(sql('SELECT * FROM _sqlx_migrations',runtime=True,check=False).returncode,0)
        self.assertEqual(sql('SELECT count(*) FROM users',runtime=True).returncode,0)
        library=self.create_library();p,_=self.register(library);self.upload(p,jpeg())
        sequences=sql("SELECT seq FROM changes WHERE library_id='"+str(uuid.UUID(library))+"' ORDER BY seq").stdout.splitlines();self.assertEqual(sequences,['1','2','3'])
        self.assertEqual(int(sql("SELECT count(*) FROM pg_roles WHERE rolname IN ('lightcraft_api','lightcraft_auth','lightcraft_migrator') AND NOT rolsuper AND NOT rolcreatedb AND NOT rolcreaterole").stdout.strip()),3)
    def test_z_container_recreation_and_crash_reconciliation(self):
        library=self.create_library();data=dng();p,_=self.register(library,data,'persist.dng','application/octet-stream');self.upload(p,data)
        # Simulate a process dying after durable bytes but before the DB commit. Keep the exact
        # final file + manifest, restore the committed pending intent and corresponding reservation.
        intent,_=self.register(library,jpeg());self.upload(intent,jpeg())
        charge=intent['byte_length']+65536
        sql("BEGIN; UPDATE photos SET status='pending',stored_at=NULL,revision=1 WHERE id='"+str(uuid.UUID(intent['id']))+"'; DELETE FROM changes WHERE entity_id='"+str(uuid.UUID(intent['id']))+"' AND kind='photo.ready'; UPDATE libraries SET change_seq=change_seq-1 WHERE id='"+str(uuid.UUID(library))+"'; UPDATE storage_budget SET used_bytes=used_bytes-"+str(charge)+",reserved_bytes=reserved_bytes+"+str(charge)+",pending_count=pending_count+1; COMMIT;")
        # Recreate every long-running container without removing named volumes.
        docker('stop','lc-api','lc-auth','lc-db')
        docker('up','-d','--no-build','--force-recreate','--wait','lc-db','lc-auth','lc-api')
        self.assertEqual(parsed('GET',API+'/health/ready')[0],200)
        for original,expected in [(p,data),(intent,jpeg())]:
            status,_,current=parsed('GET',API+'/v1/photos/'+original['id'],token=self.token());self.assertEqual(status,200);self.assertEqual(current['status'],'ready')
            status,_,stored=request('GET',API+'/v1/photos/'+original['id']+'/original',token=self.token());self.assertEqual(status,200);self.assertEqual(stored,expected)
        report={'owner_subject':self.users[0]['id'],'owner_username':self.users[0]['name'],'library_id':library,'photo_id':p['id'],'sha256':hashlib.sha256(data).hexdigest(),'postgres_and_originals_survived_container_recreation':True,'pending_final_file_reconciled':True}
        results=ROOT/'test-results';results.mkdir(exist_ok=True);(results/'persistence.json').write_text(json.dumps(report,indent=2)+'\n')

if __name__=='__main__':
    unittest.main(verbosity=2)
