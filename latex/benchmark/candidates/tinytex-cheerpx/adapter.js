const q = s => "'" + s.replaceAll("'", "'\\''") + "'";
const encode = bytes => { let s=''; for(let i=0;i<bytes.length;i+=16384) s+=String.fromCharCode(...bytes.subarray(i,i+16384)); return btoa(s); };
try {
  const CheerpX = await import('https://cxrtnc.leaningtech.com/1.2.8/cx.esm.js');
  const disk = await CheerpX.HttpBytesDevice.create(location.origin+'/assets/rootfs.ext2');
  const cache = await CheerpX.IDBDevice.create('cx-disk');
  const overlay = await CheerpX.OverlayDevice.create(disk,cache);
  const work = await CheerpX.IDBDevice.create('cx-work');
  const input = await CheerpX.DataDevice.create();
  const cx = await CheerpX.Linux.create({mounts:[
    {type:'ext2',path:'/',dev:overlay}, {type:'dir',path:'/export',dev:work},
    {type:'dir',path:'/input',dev:input}, {type:'devs',path:'/dev'}
  ]});
  const decoder=new TextDecoder();
  cx.setCustomConsole(bytes=>{vm.serial=(vm.serial+decoder.decode(bytes,{stream:true})).slice(-200000);},120,40);
  const run=command=>cx.run('/bin/sh',['-c',command],{env:['PATH=/opt/tinytex/bin/i386-linux:/usr/bin:/bin','LC_ALL=C.UTF-8','HOME=/tmp'],cwd:'/work',uid:1000,gid:100});
  globalThis.vmSend = data => { (async()=>{
    const start=performance.now();
    try {
      if(data.type==='command') {
        const result=await run(data.command);
        vm.commands[data.id]={exitCode:result.status,milliseconds:performance.now()-start};
      } else if(data.type==='write') {
        vm.activity='writeFile '+data.path;
        await input.writeFile('/'+data.id,new Uint8Array(data.bytes));
        vm.activity='cat '+data.path;
        const result=await run('cat '+q('/input/'+data.id)+' > '+q(data.path));
        if(result.status) throw new Error('Copy failed: '+result.status);
        vm.files[data.id]={written:true};
      } else {
        const copied=await run('cat '+q(data.path)+' > '+q('/export/'+data.id));
        if(copied.status) throw new Error('Missing '+data.path);
        const blob=await work.readFileAsBlob('/'+data.id);
        if(!blob) throw new Error('Missing '+data.path);
        vm.files[data.id]={bytes:encode(new Uint8Array(await blob.arrayBuffer()))};
      }
    } catch(error) { (data.type==='command'?vm.commands:vm.files)[data.id]={error:String(error)}; }
  })(); };
  vm.status='ready';
  document.querySelector('#status').textContent='Ready. Powered by CheerpX by Leaning Technologies.';
} catch(error) {vm.error=String(error);vm.status='error';}
