extern crate tch;

pub trait MilkshakeOptimizer {
    fn new(xstart: &tch::Tensor) -> Box<dyn MilkshakeOptimizer> where Self: Sized;
    fn ask(&mut self);
    fn tell(&mut self, solutions: tch::Tensor, function_values: tch::Tensor);
    fn stop(&mut self) -> tch::Tensor;

    fn disp(&mut self) {
        println!("Display not implemented for this optimizer: {}", std::any::type_name::<Self>())
    }
}