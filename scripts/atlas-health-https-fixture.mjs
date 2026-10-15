import https from 'node:https';
import fs from 'node:fs';
import path from 'node:path';
const directory=process.argv[2];
let hits=0;
const server=https.createServer({cert:fs.readFileSync(path.join(directory,'cert.pem')),key:fs.readFileSync(path.join(directory,'key.pem'))},(request,response)=>{
  fs.writeFileSync(path.join(directory,'hits.json'),JSON.stringify({hits:++hits,url:request.url,method:request.method,tls:request.socket.getProtocol()}));
  setTimeout(()=>{response.writeHead(204,{'Connection':'close'});response.end();},25);
});
server.listen(0,'127.0.0.1',()=>fs.writeFileSync(path.join(directory,'ready.json'),JSON.stringify({port:server.address().port})));
setTimeout(()=>{server.closeAllConnections();server.close();},60_000).unref();
