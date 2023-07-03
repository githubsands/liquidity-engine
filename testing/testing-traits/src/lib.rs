use crossbeam_channel::{Receiver, Sender};

pub trait ProducerDefault<'a, T: Sized, U: 'a> {
    fn producer_default() -> (Box<Self>, Receiver<U>);
}

pub trait ConsumerDefault<'a, T: Sized, U: 'a> {
    fn consumer_default() -> (Box<Self>, Sender<U>);
}
