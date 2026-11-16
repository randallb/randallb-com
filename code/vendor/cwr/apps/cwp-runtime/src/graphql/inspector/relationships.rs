#[derive(SimpleObject)]
#[graphql(complex)]
pub(super) struct InvocationCall {
    #[graphql(skip)] service:ReadService,
    #[graphql(skip)] caller_ref:String,
    #[graphql(skip)] callee_ref:Option<String>,
    call_id:String,
}
#[ComplexObject]
impl InvocationCall {
    async fn caller(&self)->Result<Option<super::invocations::ActionInvocation>> {
        super::messages::invocation(&self.service,Some(&self.caller_ref)).await
    }
    async fn callee(&self)->Result<Option<super::invocations::ActionInvocation>> {
        super::messages::invocation(&self.service,self.callee_ref.as_deref()).await
    }
}
#[derive(Clone,Copy,PartialEq,Eq,Enum)]
pub(super) enum InvocationWaitState { Pending,Ready,Resuming,Consumed,Abandoned }
impl From<cwr::v2026_09::invocation::InvocationWaitState> for InvocationWaitState {
    fn from(state:cwr::v2026_09::invocation::InvocationWaitState)->Self {
        use cwr::v2026_09::invocation::InvocationWaitState as S;
        match state {S::Pending=>Self::Pending,S::Ready=>Self::Ready,S::Resuming=>Self::Resuming,S::Consumed=>Self::Consumed,S::Abandoned=>Self::Abandoned}
    }
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub(super) struct InvocationWait {
    #[graphql(skip)] service:ReadService,
    #[graphql(skip)] waiter_ref:String,
    #[graphql(skip)] awaited_ref:Option<String>,
    call_id:String,state:InvocationWaitState,
}
#[ComplexObject]
impl InvocationWait {
    async fn waiter(&self)->Result<Option<super::invocations::ActionInvocation>> {
        super::messages::invocation(&self.service,Some(&self.waiter_ref)).await
    }
    async fn awaited(&self)->Result<Option<super::invocations::ActionInvocation>> {
        super::messages::invocation(&self.service,self.awaited_ref.as_deref()).await
    }
}
#[derive(SimpleObject)]
#[graphql(complex)]
pub(super) struct FileChange {
    #[graphql(skip)] service:ReadService,
    #[graphql(skip)] invocation:String,
    #[graphql(skip)] activity:String,
    #[graphql(skip)] key:Key,
    path:String,path_truncated:bool,change_kind:String,
}
#[ComplexObject]
impl FileChange {
    async fn diff(&self)->Result<Option<TextPreview>> {
        text(self.service.clone(),self.invocation.clone(),self.activity.clone(),Some(self.key.clone()),TextField::Diff).await
    }
}
async fn files(service:ReadService,invocation:String,activity:String,window:Window)->Result<Option<FileChangeConnection>> {
    let scope=scope(&service,&invocation,&format!("files:{activity}"))?;
    let request=request(window,&scope)?;
    let current=service.clone();let owner=invocation.clone();let reference=activity.clone();
    let Some(page)=blocking(service.reads.clone(),service.budget(),move || {
        current.publications.read(&current,|v|v.inspector_files(&owner,&reference,&request))
    }).await? else {return Ok(None);};
    let page_info=info(&page,&scope)?;let coverage=coverage(&page);
    let edges=page.entries.into_iter().map(|(key,entry)| {
        let Entry::File{path,path_truncated,change_kind}=entry else {return Err("Invalid file projection".into());};
        Ok(FileChangeEdge{cursor:cursor(&scope,&key)?,node:FileChange{service:service.clone(),invocation:invocation.clone(),activity:activity.clone(),key,path,path_truncated,change_kind}})
    }).collect::<Result<Vec<_>>>()?;
    Ok(Some(FileChangeConnection{edges,page_info,coverage}))
}
