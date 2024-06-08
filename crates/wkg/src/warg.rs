use clap::Args;

#[derive(Args, Debug)]
pub struct GetArgs {
    // TODO: Args
}

#[derive(Args, Debug)]
pub struct PushArgs {
    // TODO: Args
}

impl GetArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        todo!()
    }
}

impl PushArgs {
    pub async fn run(self) -> anyhow::Result<()> {
        todo!()
    }
}
